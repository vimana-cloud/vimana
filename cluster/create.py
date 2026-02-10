"""Create a new Vimana cluster."""

from contextlib import ExitStack
from copy import deepcopy
from datetime import datetime, timedelta
from json import dumps as dumpsJson
from json import loads as loadsJson
from shlex import quote
from tempfile import NamedTemporaryFile
from typing import Dict

from kubernetes.client import CoreV1Api
from kubernetes.config import load_kube_config
from python.runfiles import Runfiles
from rich.prompt import Confirm
from urllib3.exceptions import MaxRetryError

from cluster.profile.loader import load as loadProfile
from cluster.profile.loader import name as profileName
from dev.lib.util import console, runLoggingStderr, step, truncateTimedelta, waitFor

runfiles = Runfiles.Create()

# Paths to tool binaries.
KOPS_PATH = runfiles.Rlocation('rules_k8s/kops.exe')
HELM_PATH = runfiles.Rlocation('rules_k8s/helm.exe')
OPERATOR_DEPLOY_PATH = runfiles.Rlocation('_main/operator/deploy')

ENVOY_GATEWAY_HELM_CHART_PATH = runfiles.Rlocation(
    'rules_k8s/envoy-gateway-helm-export/gateway-helm'
)
CERT_MANAGER_HELM_CHART_PATH = runfiles.Rlocation(
    'rules_k8s/cert-manager-helm-export/cert-manager'
)

# Path to the script template used to fetch the `vimanad` runtime binary on Vimana worker nodes.
VIMANAD_FETCH_TEMPLATE_PATH = runfiles.Rlocation('_main/cluster/vimanad-fetch.sh.tmpl')

# Path at which to install the Vimana runtime on Vimana-enabled worker nodes.
VIMANAD_DESTINATION = '/usr/bin/vimanad'
# Path at which to install the Vimana runtime fetcher script
# on Vimana-enabled worker nodes.
VIMANAD_FETCH_DESTINATION = '/usr/local/bin/vimanad-fetch.sh'


def main():
    profile = loadProfile()

    # TODO: Also support other cloud platforms.
    if 'gcp' in profile:
        gcp(profile)


def gcp(profile: Dict[str, object]):
    create(profile, '--cloud=gce', f'--project={profile["gcp"]["project"]}')


def create(profile: Dict[str, object], *args):
    start = datetime.now()
    name = profileName()
    stateStore = profile['state-store']

    with ExitStack() as exitStack:
        with step(f'Provisioning cluster [bold]{name}[/bold] using [bold]kops[/bold]'):
            resourcesStream = runLoggingStderr(
                KOPS_PATH,
                'create',
                'cluster',
                name,
                *args,
                f'--state={stateStore}',
                f'--zones={profile["zone"]}',
                '--control-plane-count=1',
                f'--control-plane-size={profile["machine-type"]}',
                '--node-count=1',
                f'--node-size={profile["machine-type"]}',
                '--networking=kubenet',
                '--kubernetes-feature-gates=+RuntimeClassInImageCriApi',
                # '--topology=private',
                # '--bastion',
                '--dry-run',
                '--output=json',
            )
            # The output of `kops create cluster` using `--output=json`
            # is neither valid JSON nor NDJSON. Rather, it's comma-delimited JSON,
            # so wrapping it in square brackets makes it a valid JSON array.
            resources = loadsJson(f'[{resourcesStream.read()}]')

            # Add an additional instance group for Vimana nodes based on the default worker node.
            # Vimana-enabled nodes will be used to schedule all Vimana components,
            # while the default worker nodes will be used for Envoy Gateway pods.
            [ociWorkerNodes] = filter(
                (
                    lambda resource: (
                        resource['kind'] == 'InstanceGroup'
                        and resource['spec']['role'] == 'Node'
                    )
                ),
                resources,
            )
            workerNodeName = ociWorkerNodes['metadata']['name']
            ociWorkerNodes['metadata']['name'] = f'oci-{workerNodeName}'
            vimanaWorkerNodes = deepcopy(ociWorkerNodes)
            vimanaWorkerNodes['metadata']['name'] = f'vimana-{workerNodeName}'
            resources.append(vimanaWorkerNodes)

            # On Vimana-enabled worker nodes,
            # fetch the Vimana runtime from an artifact registry using a script.
            with open(VIMANAD_FETCH_TEMPLATE_PATH, 'r') as vimanadFetchTemplate:
                # Use Go-style template syntax for consistency with the rest of the repo
                # and because it works better with Bash scripts with many curly braces.
                vimanadFetchScript = (
                    vimanadFetchTemplate.read()
                    .replace(
                        '{{ .RepositoryUri }}',
                        quote(profile['runtime-uri']),
                    )
                    .replace(
                        '{{ .Destination }}',
                        quote(VIMANAD_DESTINATION),
                    )
                )
            vimanaWorkerNodes['spec']['fileAssets'] = [
                {
                    'name': 'vimanad-fetch.sh',
                    'path': VIMANAD_FETCH_DESTINATION,
                    'content': vimanadFetchScript,
                    'mode': '0755',
                },
            ]
            # Use hooks to configure the instance group.
            # https://kops.sigs.k8s.io/cluster_spec/#hooks
            # https://pkg.go.dev/k8s.io/kops@v1.34.1/pkg/apis/kops/v1alpha2#InstanceGroupSpec
            vimanaWorkerNodes['spec']['hooks'] = [
                # Download the Vimana runtime.
                {
                    'name': 'vimanad-fetch.service',
                    'useRawManifest': True,
                    'manifest': '\n'.join(
                        [
                            '[Unit]',
                            'Description=Fetch the Vimana runtime from the registry',
                            'After=network.target',
                            'Before=kubelet.service',
                            '',
                            '[Service]',
                            'Type=oneshot',
                            # The service should be considered "active" after the script finishes.
                            'RemainAfterExit=yes',
                            f'ExecStart=/usr/bin/bash {VIMANAD_FETCH_DESTINATION}',
                        ]
                    ),
                },
                # Run the Vimana runtime.
                {
                    'name': 'vimanad.service',
                    'useRawManifest': True,
                    'manifest': '\n'.join(
                        [
                            '[Unit]',
                            'Description=Vimana runtime',
                            'After=vimanad-fetch.service',
                            'Requires=vimanad-fetch.service',
                            'Before=kubelet.service',
                            '',
                            '[Service]',
                            'Type=exec',
                            "ExecStart=/usr/bin/bash -c '{}'".format(
                                'exec vimanad --network-interface="$({})"'.format(
                                    'ip -json route show default | jq --raw-output ".[0].dev"'
                                )
                            ),
                        ]
                    ),
                },
                # Re-configure kubectl to communicate with `vimanad` instead of `containerd`.
                {
                    'name': 'runtime-swap.service',
                    'useRawManifest': True,
                    'manifest': '\n'.join(
                        [
                            '[Unit]',
                            'Description=Connect kubelet to vimanad instead of containerd',
                            'After=vimanad.service',
                            'Before=kubelet.service',
                            '',
                            '[Service]',
                            'Type=oneshot',
                            # The service should be considered "active" after the script finishes.
                            'RemainAfterExit=yes',
                            # Assume that the `kubelet.service` file installed by nodeup includes
                            # the line:
                            #     EnvironmentFile=/etc/sysconfig/kubelet
                            # and that file sets `DAEMON_ARGS` with
                            # `--container-runtime-endpoint=unix:///run/containerd/containerd.sock`,
                            # which should be the only occurence of the containerd socket path.
                            'ExecStart=/usr/bin/sed --in-place'
                            + " 's@/run/containerd/containerd.sock@/run/vimana/vimanad.sock@'"
                            + ' /etc/sysconfig/kubelet',
                            # Don't start kubelet until the socket it ready.
                            # Wait at most 15 seconds.
                            # If the runtime doesn't open the socket by then,
                            # surely something is amiss.
                            "ExecStartPost=/usr/bin/timeout 15s /usr/bin/bash -c '{}'".format(
                                'until [ -S /run/vimana/vimanad.sock ]; do sleep 0.1; done'
                            ),
                        ]
                    ),
                },
            ]
            # Add a label and taint so Vimana workloads get scheduled here
            # and gateway-related workloads do not.
            vimanaWorkerNodes['spec']['nodeLabels'] = {'runtime': 'vimanad'}
            vimanaWorkerNodes['spec']['taints'] = ['runtime=vimanad:NoSchedule']

            with NamedTemporaryFile('w+', suffix='.yaml') as resourceFile:
                # YAML is a superset of JSON, so we can just serialize each resource as JSON
                # and delimit them with YAML document separators.
                resourceFile.write(
                    '\n---\n'.join(
                        dumpsJson(resource, separators=(',', ':'))
                        for resource in resources
                    )
                )
                resourceFile.flush()

                runLoggingStderr(
                    KOPS_PATH,
                    'create',
                    f'--state={stateStore}',
                    f'--filename={resourceFile.name}',
                )

            # Offer to clean up the cluster if anything fails
            # between starting to provision it and successfully finishing.
            @exitStack.callback
            def cleanup():
                if Confirm.ask('Clean up partially-initialized cluster?'):
                    with step(
                        f'Deleting cluster [bold]{name}[/bold] using [bold]kops[/bold]'
                    ):
                        runLoggingStderr(
                            KOPS_PATH,
                            'delete',
                            'cluster',
                            name,
                            f'--state={stateStore}',
                            '--yes',
                        )

            runLoggingStderr(
                KOPS_PATH,
                'update',
                'cluster',
                name,
                f'--state={stateStore}',
                '--admin',
                '--yes',
            )

        load_kube_config()
        coreApi = CoreV1Api()

        def controlPlaneReady():
            try:
                coreApi.get_api_resources()
                return True
            except MaxRetryError:
                return False

        waitFor(
            controlPlaneReady,
            'the control plane to become ready',
            timeout=timedelta(minutes=5),
            interval=timedelta(seconds=10),
            minimum=timedelta(minutes=1),
        )

        def ociWorkerNodeReady():
            # Check for at least one worker node without the Vimana taint,
            # so Envoy Gateway pods can be scheduled in the next step.
            nodes = coreApi.list_node()
            for node in nodes.items:
                # Skip control plane nodes and nodes with the Vimana taint.
                if (
                    'node-role.kubernetes.io/control-plane' not in node.metadata.labels
                    and all(
                        taint.key != 'runtime' or taint.value != 'vimanad'
                        for taint in (node.spec.taints or [])
                    )
                    and any(
                        condition.type == 'Ready' and condition.status == 'True'
                        for condition in node.status.conditions
                    )
                ):
                    return True
            return False

        with step('Installing Envoy Gateway using [bold]helm[/bold]'):
            runLoggingStderr(
                HELM_PATH,
                'install',
                'envoy-gateway',
                ENVOY_GATEWAY_HELM_CHART_PATH,
                # Use gateway namespace mode
                # to create load balancer services in the same namespace as the Gateway resource.
                # https://gateway.envoyproxy.io/docs/tasks/operations/gateway-namespace-mode/
                '--set=config.envoyGateway.provider.kubernetes.deploy.type=GatewayNamespace',
                # Enable EnvoyPatchPolicy for gRPC-JSON transcoding support.
                # https://gateway.envoyproxy.io/latest/tasks/extensibility/envoy-patch-policy/
                '--set=config.envoyGateway.extensionApis.enableEnvoyPatchPolicy=true',
                '--namespace=envoy-gateway-system',
                '--create-namespace',
            )

        with step('Installing [bold]cert-manager[/bold] using [bold]helm[/bold]'):
            runLoggingStderr(
                HELM_PATH,
                'install',
                'cert-manager',
                CERT_MANAGER_HELM_CHART_PATH,
                '--set=crds.enabled=true',
                '--set=config.enableGatewayAPI=true',
                '--namespace=cert-manager',
                '--create-namespace',
            )

        with step('Installing the Vimana operator'):
            runLoggingStderr(OPERATOR_DEPLOY_PATH)

        # Cluster creation succeeded. No cleanup necessary.
        exitStack.pop_all()

        elapsed = truncateTimedelta(datetime.now() - start)
        console.print(
            f'[bold]{name}[/bold] successfully created after [bold]{elapsed}[/bold] 🐣',
        )


if __name__ == '__main__':
    main()
