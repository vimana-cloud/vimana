"""
Push a Vimana "container" image,
consisting of a component module and matching metadata,
to an OCI container registry.
"""

from argparse import ArgumentParser
from hashlib import sha256
from urllib.parse import parse_qsl, urlencode, urlparse, urlunparse

from requests import post, put

from dev.lib.util import codeMessage, console


def main(
    registry: str,
    domain: str,
    server: str,
    version: str,
    component: bytes,
    metadata: bytes,
):
    # Push the component and metadata blobs.
    componentDigest = pushBlob(registry, domain, server, component)
    metadataDigest = pushBlob(registry, domain, server, metadata)

    # Create and push the image config blob.
    # https://specs.opencontainers.org/image-spec/config/#properties
    # These are the minimum required properties, and they're all ignored.
    imageConfig = b'{"architecture":"wasm","os":"vimana","rootfs":{"type":"layers","diff_ids":[]}}'
    imageConfigDigest = pushBlob(registry, domain, server, imageConfig)

    # Build the manifest.
    # https://specs.opencontainers.org/image-spec/manifest/#image-manifest
    manifest = {
        'schemaVersion': 2,
        # https://specs.opencontainers.org/image-spec/descriptor/
        'config': {
            'mediaType': 'application/vnd.oci.image.config.v1+json',
            'size': len(imageConfig),
            'digest': imageConfigDigest,
        },
        'layers': [
            {
                'mediaType': 'application/wasm',
                'size': str(len(component)),
                'digest': componentDigest,
            },
            {
                'mediaType': 'application/protobuf',
                'size': str(len(metadata)),
                'digest': metadataDigest,
            },
        ],
    }

    # Push the manifest.
    # https://specs.opencontainers.org/distribution-spec/#pushing-manifests
    tagUrl = f'{registry}/v2/{domain}/{server}/manifests/{version}'
    response = put(
        tagUrl,
        headers={'Content-Type': 'application/vnd.oci.image.manifest.v1+json'},
        data=manifest,
    )
    if not response.ok:
        raise RuntimeError(codeMessage(response.status_code, response.text))

    console.print(
        f'Pushed [blue]{domain}[/blue]:[yellow]{server}[/yellow]@[magenta]{version}[/magenta]'
    )


def pushBlob(registry: str, domain: str, server: str, content: bytes) -> str:
    """
    Push a blob to an OCI registry and return its digest.

    Args:
        registry: Registry URL (e.g. 'http://localhost:5000').
        domain: Domain ID (e.g. '1234567890abcdef1234567890abcdef').
        server: Server ID (e.g. 'some-server').
        content: Binary content of the blob to push.

    Returns:
        The digest of the pushed blob (e.g. 'sha256:...').
    """
    # https://specs.opencontainers.org/distribution-spec/#pushing-blobs
    postUrl = f'{registry}/v2/{domain}/{server}/blobs/uploads/'

    # Follow redirects, fail on non-200-range status code,
    # and extract the value of the `Location` header.
    response = post(postUrl, allow_redirects=True)
    if not response.ok:
        raise RuntimeError(codeMessage(response.status_code, response.text))

    putLocation = response.headers.get('Location')
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

    response = put(
        putUrl,
        headers={
            'Content-Type': 'application/octet-stream',
            'Content-Length': str(len(content)),
        },
        data=content,
    )
    if not response.ok:
        raise RuntimeError(codeMessage(response.status_code, response.text))

    return digest


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        '--registry',
        required=True,
        metavar='URL',
        help="Registry URL (e.g. 'http://localhost:5000')",
    )
    parser.add_argument(
        '--domain',
        required=True,
        metavar='ID',
        help="Domain ID (e.g. '1234567890abcdef1234567890abcdef')",
    )
    parser.add_argument(
        '--server',
        required=True,
        metavar='ID',
        help="Server ID (e.g. 'some-server')",
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
        registry=args.registry,
        domain=args.domain,
        server=args.server,
        version=args.version,
        component=component,
        metadata=metadata,
    )
