"""
Push a Vimana "container" image,
consisting of a component module and matching metadata,
to an OCI container registry.
"""

from argparse import ArgumentParser
from datetime import UTC, datetime
from hashlib import sha256
from json import dumps
from typing import Dict
from urllib.parse import parse_qsl, urlencode, urlparse, urlunparse

from dev.lib.util import console, requestOrDie


def main(
    repository: str,
    version: str,
    component: bytes,
    metadata: bytes,
):
    # Parse the repository URL to extract registry (scheme + host) and namespace (path).
    parsed = urlparse(repository)
    registry = f'{parsed.scheme}://{parsed.netloc}'
    namespace = parsed.path.lstrip('/')

    # Push the component and metadata blobs.
    componentDigest = pushBlob(registry, namespace, component)
    metadataDigest = pushBlob(registry, namespace, metadata)

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
    imageConfigDigest = pushBlob(registry, namespace, imageConfig)

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
        headers={'Content-Type': 'application/vnd.oci.image.manifest.v1+json'},
        data=serializeJson(manifest),
    )

    console.print(f'Pushed {repository}:{version}')


def pushBlob(registry: str, namespace: str, content: bytes) -> str:
    """
    Push a blob to an OCI repository and return its digest.

    Args:
        registry: Registry hostname and scheme (e.g. 'http://localhost:5000').
        namespace: Repository namespace (e.g. 'path/to/image').
        server: Server ID (e.g. 'some-server').
        content: Binary content of the blob to push.

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
        headers={
            'Content-Type': 'application/octet-stream',
            'Content-Length': str(len(content)),
        },
        data=content,
    )

    return digest


def serializeJson(json: Dict[str, any]) -> bytes:
    """Helper method to serialize a JSON object as compact, binary data."""
    return dumps(json, separators=(',', ':')).encode()


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        '--repository',
        required=True,
        metavar='URL',
        help="Repository URL including scheme (e.g. 'https://docker.io/path/to/image')",
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
    )
