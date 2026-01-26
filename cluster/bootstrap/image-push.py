"""
Push a Vimana "container" image,
consisting of a component module and matching metadata,
to an OCI container registry.
"""

from argparse import ArgumentParser
from base64 import b64decode, b64encode
from contextlib import AbstractAsyncContextManager, contextmanager
from datetime import UTC, datetime
from hashlib import sha256
from json import dumps as dumpsJson
from json import loads as loadsJson
from os import environ, getenv
from os.path import exists as pathExists
from os.path import join as joinPath
from shutil import copytree
from tempfile import TemporaryDirectory
from typing import Dict, Set
from urllib.parse import parse_qsl, urlencode, urlparse, urlunparse

from dev.lib.util import console, requestOrDie, runOrDie


def main(
    repository: str,
    version: str,
    component: bytes,
    metadata: bytes,
    insecureRegistries: Set[str],
):
    # Parse the repository URL to determine the registry domain (`netloc`) and namespace (`path`).
    # Use an empty scheme so it parses properly.
    parsed = urlparse(f'//{repository}')
    scheme = 'http' if parsed.netloc in insecureRegistries else 'https'
    registry = f'{scheme}://{parsed.netloc}'
    namespace = parsed.path.lstrip('/')

    # Determine extra headers (for auth) based on the registry hostname.
    headers = authorityHeaders(parsed.netloc)

    # Push the component and metadata blobs.
    componentDigest = pushBlob(registry, namespace, component, headers=headers)
    metadataDigest = pushBlob(registry, namespace, metadata, headers=headers)

    # Create and push the image config blob,
    # adhering as closely to the Wasm OCI artifact spec as we can.
    # https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/#configmediatype-applicationvndwasmconfigv0json
    # https://specs.opencontainers.org/image-spec/config/#properties
    imageConfig = {
        'created': datetime.now(UTC).strftime('%Y-%m-%dT%H:%M:%S.%fZ'),
        'architecture': 'wasm',
        # This setting for `os` indicates non-compliance with the Wasm OCI spec,
        # because the Protobuf-encoded metadata layer is obviously not a Wasm component.
        # Perhaps we can instead package it as a "runtime configuration" or "static files".
        # https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/#faq
        'os': 'vimana',
        'layerDigests': [componentDigest, metadataDigest],
        # TODO: Parse the compiled component for import / export information.
        'component': {},
    }
    imageConfig = serializeJson(imageConfig)
    imageConfigDigest = pushBlob(registry, namespace, imageConfig, headers=headers)

    # Build the manifest.
    # https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/#manifest-format
    # https://specs.opencontainers.org/image-spec/manifest/#image-manifest
    manifest = {
        'schemaVersion': 2,
        'mediaType': 'application/vnd.oci.image.manifest.v1+json',
        # https://specs.opencontainers.org/image-spec/descriptor/
        'config': {
            'mediaType': 'application/vnd.wasm.config.v0+json',
            'size': len(imageConfig),
            'digest': imageConfigDigest,
        },
        'layers': [
            {
                'mediaType': 'application/wasm',
                'size': len(component),
                'digest': componentDigest,
            },
            {
                'mediaType': 'application/protobuf',
                'size': len(metadata),
                'digest': metadataDigest,
            },
        ],
    }

    # Push the manifest.
    # https://specs.opencontainers.org/distribution-spec/#pushing-manifests
    tagUrl = f'{registry}/v2/{namespace}/manifests/{version}'
    requestOrDie(
        'PUT',
        tagUrl,
        headers=(
            headers | {'Content-Type': 'application/vnd.oci.image.manifest.v1+json'}
        ),
        data=serializeJson(manifest),
    )

    console.print(f'Pushed {repository}:{version}')


def pushBlob(
    registry: str, namespace: str, content: bytes, headers: Dict[str, str] = {}
) -> str:
    """
    Push a blob to an OCI repository and return its digest.

    Args:
        registry: Registry hostname and scheme (e.g. 'http://localhost:5000').
        namespace: Repository namespace (e.g. 'path/to/image').
        content: Binary content of the blob to push.
        headers: HTTP headers to include in the request.

    Returns:
        The digest of the pushed blob (e.g. 'sha256:...').
    """
    # https://specs.opencontainers.org/distribution-spec/#pushing-blobs
    postUrl = f'{registry}/v2/{namespace}/blobs/uploads/'

    # Follow redirects, fail on non-200-range status code,
    # and extract the value of the `Location` header.
    putLocation = requestOrDie(
        'POST',
        postUrl,
        allow_redirects=True,
        headers=headers,
    ).headers.get('Location')
    if not putLocation:
        raise RuntimeError(f"Response missing 'Location' header for '{postUrl}'")

    # The location MAY be relative to the same origin,
    # in which case we must make it absolute.
    if putLocation.startswith('/'):
        putLocation = f'{registry}{putLocation}'

    digest = f'sha256:{sha256(content).hexdigest()}'

    # Add the digest as a query parameter.
    putUrlParsed = urlparse(putLocation)
    putQueryParsed = parse_qsl(putUrlParsed.query, keep_blank_values=True)
    putQueryParsed.append(('digest', digest))
    putUrl = urlunparse(
        (
            putUrlParsed.scheme,
            putUrlParsed.netloc,
            putUrlParsed.path,
            putUrlParsed.params,
            urlencode(putQueryParsed),
            putUrlParsed.fragment,
        )
    )

    requestOrDie(
        'PUT',
        putUrl,
        headers=(
            headers
            | {
                'Content-Type': 'application/octet-stream',
                'Content-Length': str(len(content)),
            }
        ),
        data=content,
    )

    return digest


def serializeJson(json: Dict[str, any]) -> bytes:
    """Helper method to serialize a JSON object as compact, binary data."""
    return dumpsJson(json, separators=(',', ':')).encode()


def authorityHeaders(authority: str) -> Dict[str, str]:
    """
    Get authorization headers for a given registry using Docker's credential system.

    Read `~/.docker/config.json` and use `credHelpers` or `auths` to obtain credentials.
    """
    configPath = joinPath(
        getenv('DOCKER_CONFIG', joinPath(getenv('HOME', '~'), '.docker')),
        'config.json',
    )
    try:
        with open(configPath, 'r') as f:
            config = loadsJson(f.read())
    except FileNotFoundError:
        return {}

    # Check for a credential helper for this registry.
    credHelper = config.get('credHelpers', {}).get(authority)
    if credHelper is not None:
        # Allow running `gcloud` in the linux-sandbox with read-only filesystem.
        with (
            fakeWritableGcloudConfig()
            if credHelper == 'gcloud'
            else AbstractAsyncContextManager()
        ):
            credentials = loadsJson(
                runOrDie(
                    [f'docker-credential-{credHelper}', 'get'],
                    input=authority,
                ).stdout
            )
        return basicAuthHeader(credentials['Username'], credentials['Secret'])

    # Fall back to hardcoded credentials, if available.
    auths = config.get('auths', {})
    # The authority may or may not be prefixed with `https://` here.
    auth = auths.get(authority) or auths.get(f'https://{authority}')
    if auth and 'auth' in auth:
        # `auth` is base64-encoded `username:password`.
        decoded = b64decode(auth['auth']).decode()
        username, password = decoded.split(':', 1)
        return basicAuthHeader(username, password)

    return {}


def basicAuthHeader(username: str, password: str) -> Dict[str, str]:
    """Create a Basic Authorization header from username and password."""
    credentials = b64encode(f'{username}:{password}'.encode()).decode()
    return {'Authorization': f'Basic {credentials}'}


@contextmanager
def fakeWritableGcloudConfig():
    """
    Set up a temporary, writeable version of the gcloud config directory.
    """
    # TODO: Delete this function if `gcloud` stops requiring a writable config directory.
    #   https://issuetracker.google.com/issues/478634528
    realConfigDir = joinPath(
        getenv(
            'CLOUDSDK_CONFIG',
            getenv('XDG_CONFIG_HOME', joinPath(getenv('HOME', '~'), '.config')),
        ),
        'gcloud',
    )
    with TemporaryDirectory() as tempDir:
        fakeConfigDir = joinPath(tempDir, 'gcloud')
        if pathExists(realConfigDir):
            copytree(realConfigDir, fakeConfigDir)
            environ['CLOUDSDK_CONFIG'] = fakeConfigDir
        yield


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        '--repository',
        required=True,
        metavar='URL',
        help="Repository URL excluding scheme (e.g. 'docker.io/path/to/image')",
    )
    parser.add_argument(
        '--version',
        required=True,
        metavar='STRING',
        help="Version string (e.g. '1.2.3-release')",
    )
    parser.add_argument(
        '--component',
        required=True,
        metavar='PATH',
        help='Path to compiled Wasm component module',
    )
    parser.add_argument(
        '--metadata',
        required=True,
        metavar='PATH',
        help='Path to serialized container metadata',
    )
    parser.add_argument(
        '--insecure-registry',
        action='append',
        default=[],
        help='Insecure registry domain (can be repeated)',
    )
    args = parser.parse_args()

    with open(args.component, 'rb') as componentFile:
        component = componentFile.read()
    with open(args.metadata, 'rb') as metadataFile:
        metadata = metadataFile.read()

    main(
        repository=args.repository,
        version=args.version,
        component=component,
        metadata=metadata,
        insecureRegistries=set(args.insecure_registry),
    )
