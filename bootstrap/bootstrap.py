"""Construct the K8s JSON resources for a Vimana domain."""

from argparse import ArgumentParser
from base64 import b64encode
from collections.abc import Callable
from functools import partial
from hashlib import sha224
from json import dump, load
from os import getenv
from os.path import join as joinPath
from subprocess import Popen
from tempfile import NamedTemporaryFile

# Name of the global Vimana gateway resource.
VIMANA_GATEWAY_NAME = 'vimana-gateway'
# Runtime class for workd.
# A pod with any other runtime class will be delegated to the downstream runtime.
WORKD_RUNTIME_CLASS = 'workd-runtime-class'
# gRPC services are exposes via this port number at the Vimana gateway.
GRPC_GATEWAY_PORT = 443
# gRPC services must always use this port number internally.
GRPC_CONTAINER_PORT = 80

# Paths to the `openssl` and `tls-generate` binaries
# used to generate private keys and certificates.
# `RUNFILES_DIR` is set when invoked via `bazel build`.
# `..` is the parent for external repo data dependencies when invoked via `bazel run`.
RUNFILES_DIR = getenv('RUNFILES_DIR', '..')
OPENSSL_PATH = joinPath(RUNFILES_DIR, 'openssl+', 'openssl')
TLS_GENERATE_PATH = joinPath(RUNFILES_DIR, 'rules_k8s+', 'tls-generate')


def bootstrap(
    domains: dict[str, object],
    clusterRegistry: str,
    makeTlsSecret: Callable[[str, str], dict[str, object]],
) -> list[dict[str, object]]:
    """
    Generate a the K8s resources necessary to bootstrap a Vimana cluster.

    Parameters:
        domains:
            The main payload for bootstrap configuration,
            passed as a dictionary blob because it comes from Starlark.
            See `bootstrap.bzl` to understand how this looks.
        clusterRegistry:
            URL base for the container image registry within the cluster.
            Used to construct image URL's for container specs.
        makeTlsSecret:
            A function that generates K8s Secret resources for TLS certificates
            based on a given resource name and certificate hostname.
    Result:
        The list of K8s resource objects necessary to bootstrap the cluster.
    """
    # List of `Listener` for the gateway (populated later in the function).
    # In order to support SNI-based dynamic TLS,
    # we'll need one listener per canonical domain name plus one listener per alias.
    listeners = []
    # List of K8s resources as JSON-serializable objects to return.
    resources = [
        {
            'kind': 'Gateway',
            'apiVersion': 'gateway.networking.k8s.io/v1',
            'metadata': {
                'name': VIMANA_GATEWAY_NAME,
            },
            'spec': {
                'gatewayClassName': 'istio',
                'listeners': listeners,
            },
        },
        {
            'kind': 'RuntimeClass',
            'apiVersion': 'node.k8s.io/v1',
            'metadata': {
                'name': WORKD_RUNTIME_CLASS,
            },
            'handler': 'workd',  # TODO: Make this the same as WORKD_RUNTIME_CLASS?
        },
    ]

    for domainId, domain in domains.items():
        hostnames = set(domain['aliases'])
        hostnames.add(f'{domainId}.app.vimana.host')

        # Create a TLS secret and associated listener for each hostname.
        for hostname in hostnames:
            # Derive valid K8s Secret and Listener names from the hostname.
            k8sSecretName = deriveName(hostname, 'c')
            k8sListenerName = deriveName(hostname, 'l')
            # https://gateway-api.sigs.k8s.io/reference/spec/#listener
            listeners.append(
                {
                    'name': k8sListenerName,
                    'protocol': 'HTTPS',
                    'port': GRPC_GATEWAY_PORT,
                    'hostname': hostname,
                    'tls': {
                        'certificateRefs': [
                            {
                                'kind': 'Secret',
                                'name': k8sSecretName,
                            },
                        ],
                    },
                    # All internal routing occurs with gRPC.
                    # JSON transcoding can be achieved by EnvoyFilter objects.
                    'allowedRoutes': {
                        'kinds': [{'kind': 'GRPCRoute'}],
                    },
                }
            )
            resources.append(makeTlsSecret(k8sSecretName, hostname))

        # List of `GRPCRouteRule` for the domain (one per service; populated later).
        grpcRouteRules = []
        # One route per domain, configured for all the domain's hostnames.
        # https://gateway-api.sigs.k8s.io/reference/spec/#grpcroute
        resources.append(
            {
                'kind': 'GRPCRoute',
                'apiVersion': 'gateway.networking.k8s.io/v1',
                'metadata': {
                    'name': domainId,
                    'labels': {
                        'vimana.host/domain': domainId,
                    },
                },
                'spec': {
                    # All routes are parented by the global gateway.
                    'parentRefs': [
                        {'name': VIMANA_GATEWAY_NAME},
                    ],
                    # One hostname for the canonical domain and one for each alias.
                    'hostnames': list(hostnames),
                    'rules': grpcRouteRules,
                },
            }
        )

        for serviceName, service in domain['services'].items():
            # List of `GRPCBackendRef` for the service (one per component; populated later).
            grpcBackendRefs = []
            # https://gateway-api.sigs.k8s.io/reference/spec/#grpcrouterule
            grpcRouteRules.append(
                {
                    'matches': [
                        {
                            'method': {
                                'service': serviceName,
                                'type': 'Exact',
                            },
                        },
                    ],
                    'backendRefs': grpcBackendRefs,
                }
            )

            for componentVersion, component in service['components'].items():
                # Derive valid K8s Service, Deployment, and Pod names
                # from the canonical component name.
                componentName = f'{domainId}:{serviceName}@{componentVersion}'
                k8sServiceName = deriveName(componentName, 's')
                k8sDeploymentName = deriveName(componentName, 'd')

                # Labels common to all component-specific resources.
                componentLabels = {
                    'vimana.host/domain': domainId,
                    'vimana.host/service': serviceName,
                    'vimana.host/version': componentVersion,
                }

                # It's called a 'Service' resource but it represents a Vimana component.
                # https://kubespec.dev/v1/Service
                resources.append(
                    {
                        'kind': 'Service',
                        'apiVersion': 'v1',
                        'metadata': {
                            'name': k8sServiceName,
                            'labels': componentLabels,
                        },
                        'spec': {
                            # Every component serves cleartext HTTP/2 (gRPC) traffic.
                            # Public TLS termination and JSON transcoding happens at the Gateway,
                            # and mTLS for mesh traffic is provided transparently by Ztunnel
                            # (Istio ambient mode).
                            'ports': [
                                {
                                    'name': 'grpc',
                                    'port': GRPC_CONTAINER_PORT,
                                    'appProtocol': 'kubernetes.io/h2c',
                                }
                            ],
                            'selector': componentLabels,
                        },
                    }
                )

                # One deployment resource per component as well.
                # https://kubespec.dev/apps/v1/Deployment
                resources.append(
                    {
                        'kind': 'Deployment',
                        'apiVersion': 'apps/v1',
                        'metadata': {
                            'name': k8sDeploymentName,
                            'labels': componentLabels,
                        },
                        'spec': {
                            'replicas': 1,
                            'selector': {
                                'matchLabels': componentLabels,
                            },
                            'template': {
                                'metadata': {
                                    'labels': componentLabels,
                                },
                                'spec': {
                                    'runtimeClassName': WORKD_RUNTIME_CLASS,
                                    'serviceAccountName': '',
                                    # Workd pods have a single container, called 'grpc'.
                                    'containers': [
                                        {
                                            'name': 'grpc',
                                            'image': '{}/{}/{}:{}'.format(
                                                clusterRegistry,
                                                domainId,
                                                _hexify(serviceName),
                                                componentVersion,
                                            ),
                                            # TODO: Determine testability implications of image pull policy.
                                            # 'imagePullPolicy': 'Always',
                                            'env': [],
                                        },
                                    ],
                                },
                            },
                        },
                    },
                )

                # https://gateway-api.sigs.k8s.io/reference/spec/#grpcbackendref
                grpcBackendRefs.append(
                    {
                        'name': k8sServiceName,
                        'kind': 'Service',
                        'port': GRPC_CONTAINER_PORT,
                        'weight': component['weight'],
                    }
                )

    return resources


def generateDomainTls(
    caKey: str, caCert: str, name: str, hostname: str
) -> dict[str, object]:
    """
    Generate a TLS private key and certificate as a K8s Secret resource.

    Parameters:
        caKey: Path to root CA's private key file (see `generateRootCa`).
        caCert: Path to root CA's certificate file (see `generateRootCa`).
        name: Name for the K8s Secret resource.
        hostname: Subject (common name) for the TLS certificate.
    Result:
        K8s Secret resource object.
    """
    # The certificate generator can only write to named output files.
    with NamedTemporaryFile() as keyFile:
        with NamedTemporaryFile() as certFile:
            command = [
                TLS_GENERATE_PATH,
                hostname,
                f'--key={keyFile.name}',
                f'--cert={certFile.name}',
                f'--root-key={caKey}',
                f'--root-cert={caCert}',
                f'--openssl={OPENSSL_PATH}',
            ]
            if Popen(command).wait() != 0:
                raise RuntimeError(
                    f"Failed to generate TLS certificate for '{hostname}'."
                )
            key = keyFile.read()
            cert = certFile.read()

    # https://kubernetes.io/docs/concepts/configuration/secret/#tls-secrets
    return {
        'kind': 'Secret',
        'apiVersion': 'v1',
        'metadata': {
            'name': name,
        },
        'type': 'kubernetes.io/tls',
        'data': {
            'tls.crt': b64encode(cert).decode(),
            'tls.key': b64encode(key).decode(),
        },
    }


def generateRootCa(pathPrefix: str) -> (str, str):
    """
    Generate a self-signed root CA private key and certificate as 2 PEM-encoded files.

    Parameters:
        pathPrefix:
            Path prefix for the output files.
            The private key and certificate will be given suffixes
            `.key` and `.cert`, respectively.
    Result:
        Paths to the key and certificate file (in that order).
    """
    keyPath = f'{pathPrefix}.key'
    certPath = f'{pathPrefix}.cert'

    command = [
        TLS_GENERATE_PATH,
        '--ca',
        f'--key={keyPath}',
        f'--cert={certPath}',
        f'--openssl={OPENSSL_PATH}',
    ]
    if Popen(command).wait() != 0:
        raise RuntimeError(f'Failed to generate root CA.')

    return (keyPath, certPath)


def deriveName(name: str, prefix: str) -> str:
    """
    Return a derivative name that's guaranteed to be a valid DNS label.

    Many names in K8s are required to be a valid DNS label:
    63 character max length, alphanumeric and hyphens, starting with a letter.
    To guarantee conformance while reasonably ensuring uniqueness,
    take the SHA-224 hash of a truly unique name and add a known-letter prefix.

    Parameters:
        name: A truly unique name within some namespace.
        prefix:
            A one-letter prefix to ensure that the first character is a letter.
            Also acts as a type hint / namespace indicator.
    Result:
        Derivative name that's guaranteed to be a valid DNS label.
    """
    hasher = sha224()
    hasher.update(name.encode())
    return f'{prefix}-{hasher.hexdigest()}'


def _hexify(name: str):
    """Return the nibblewise little-endian hex encoding of the given string."""
    # Compute nibblewise big-endian hex.
    nbeHex = name.encode().hex()
    # Swap each nibble pair.
    return ''.join(little + big for little, big in zip(nbeHex[1::2], nbeHex[::2]))


description = """
Generate a file containing the Kubernetes resources needed to bootstrap a Vimana cluster.
Optionally generate a private key and self-signed certificate for a root CA
used to sign all the TLS certificates used in the cluster.
""".strip()

epilog = """
The main input to this program is a JSON-encoded configuration file
describing the domains, services, and components to bootstrap.
See `bootstrap.bzl` to understand how the configuration should look.
""".strip()

if __name__ == '__main__':
    parser = ArgumentParser(description=description, epilog=epilog)
    parser.add_argument('config', help='Path to the input configuration file')
    parser.add_argument('resources', help='Path to the output resources file')
    parser.add_argument(
        '--registry',
        required=True,
        metavar='URL',
        help='Container image registry base',
    )
    parser.add_argument(
        '--generate-ca',
        metavar='PATH',
        help='Generate a CA private key and self-signed certificate '
        + 'to sign all domain TLS certificates, '
        + 'stored at the given path with suffixes `.key` and `.cert` respectively',
    )
    args = parser.parse_args()

    with open(args.config, 'r') as config:
        domains = load(config)

    if args.generate_ca is None:
        # TODO: Support this.
        raise NotImplementedError('Using an external CA is not currently supported.')
    caKey, caCert = generateRootCa(args.generate_ca)
    makeTlsSecret = partial(generateDomainTls, caKey, caCert)

    resources = bootstrap(domains, args.registry, makeTlsSecret)

    with open(args.resources, 'w') as file:
        for resource in resources:
            dump(resource, file, indent=2)
            file.write('\n')
