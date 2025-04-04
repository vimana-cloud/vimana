#!/usr/bin/env bash

# Push a Vimana "container" image,
# consisting of a component module and matching metadata,
# to an OCI container registry.

registry="$1"   # e.g. `http://localhost:5000`.
domain="$2"     # e.g. `1234567890abcdef1234567890abcdef`.
service="$3"    # e.g. `some.package.FooService`.
version="$4"    # e.g. `1.0.0-release`.
component="$5"  # Compiled Wasm component module path.
metadata="$6"   # Serialized container metadata path.

# Repository namespace components must contain only lowercase letters and digits,
# so use `od` to hex-encode the service.
# Flip each pair of nibbles to make it nibble-wise little-endian
# because that's how workd happens to work.
service_hex="$(echo -n "$service" | od -A n -t x1 | tr -d " \n" | sed 's/\(.\)\(.\)/\2\1/g')"

# $1: Path to file containing blob to push.
function push-blob {
  path="$1"

  # https://specs.opencontainers.org/distribution-spec/#pushing-blobs
  post_url="${registry}/v2/${domain}/${service_hex}/blobs/uploads/"
  # Follow redirects, fail on non-200-range status code,
  # and extract the value of the `Location` header.
  # Note that HTTP/1.1 response headers
  # always end in carriage-return (`\r`) then newline (`$`).
  put_location="$(curl -X POST --dump-header - --silent --location --fail "$post_url" \
                  | sed -n 's/^Location: \(.*\)\r$/\1/p')" || {
    echo >&2 "Error posting '$post_url'"
    return 1
  }

  # If the location already includes a query component,
  # append the digest with `&`. Otherwise, append it with `?`.
  [[ "$put_location" = *\?* ]] && digest_separator='&' || digest_separator='?'
  # The location MAY be relative, in which case we must make it absolute.
  [[ "$put_location" = /* ]] && put_location="${registry}${put_location}"

  # `sha256sum` annoyingly prints the filename after the hash,
  # so only keep the first 64 hexadecimal characters (representing 32 octets = 256 bits).
  digest="sha256:$(sha256sum "$path" | head --bytes=64)"
  put_url="${put_location}${digest_separator}digest=${digest}"
  curl -X PUT --silent --location --fail "$put_url" \
      -H "Content-Length: $(< "$path" wc -c)" \
      -H "Content-Type: application/octet-stream" \
      --data-binary "@$path" || {
    echo >&2 "Error putting '$put_url'"
    return 2
  }

  # Print the digest to "return" it.
  echo "$digest"
}

component_digest="$(push-blob "$component")" || exit $?
metadata_digest="$(push-blob "$metadata")" || exit $?

# https://specs.opencontainers.org/image-spec/config/#properties
# These are the minimum required properties, and they're all ignored.
image_config="$(mktemp)"
# Delete the teporary file on exit.
function delete-image-config {
  rm "$image_config"
}
trap delete-image-config EXIT

echo -n '{"architecture":"wasm","os":"workd","rootfs":{"type":"layers","diff_ids":[]}}' \
  > "$image_config"
image_config_digest="$(push-blob "$image_config")" || exit $?

# Print a descriptor object:
# https://specs.opencontainers.org/image-spec/descriptor/.
#
# $1: Path to the blob file.
# $2: Digest of the blob.
# $3: MIME media type.
function print-descriptor {
  echo -n "{\"mediaType\":\"$3\","
  echo -n  "\"size\":$(< "$1" wc -c),"
  echo -n  "\"digest\":\"$2\"}"
}

manifest="$(mktemp)"
# Delete all teporary files on exit (overwrites previous trap).
function delete-temporary-files {
  rm "$image_config" "$manifest"
}
trap delete-temporary-files EXIT

# https://specs.opencontainers.org/image-spec/manifest/#image-manifest
# Should always result in something that looks like this:
#     {
#         'schemaVersion': 2,
#         'config': {
#             'mediaType': 'application/vnd.oci.image.config.v1+json',
#             'size': len(imageConfig),
#             'digest': imageConfigDigest,
#         },
#         'layers': [
#             {
#                 'mediaType': 'application/wasm',
#                 'size': len(component),
#                 'digest': componentDigest,
#             },
#             {
#                 'mediaType': 'application/protobuf',
#                 'size': len(metadata),
#                 'digest': metadataDigest,
#             },
#         ],
#     }
{
  echo -n '{"schemaVersion":2,"config":'
  print-descriptor "$image_config" "$image_config_digest" 'application/vnd.oci.image.config.v1+json'
  echo -n ',"layers":['
  print-descriptor "$component" "$component_digest" 'application/wasm'
  echo -n ','
  print-descriptor "$metadata" "$metadata_digest" 'application/protobuf'
  echo -n ']}'
} > "$manifest"

# https://specs.opencontainers.org/distribution-spec/#pushing-manifests
put_url="${registry}/v2/${domain}/${service_hex}/manifests/${version}"
curl -X PUT --silent --location --fail "$put_url" \
    -H "Content-Type: application/vnd.oci.image.manifest.v1+json" \
    --data-binary "@$manifest" || {
  echo >&2 "Error putting '$put_url'"
  exit 3
}

# Format output only if stderr (2) is a terminal (-t).
if [ -t 2 ]
then
  # https://en.wikipedia.org/wiki/ANSI_escape_code
  reset='\033[0m' # No formatting.
  bold='\033[1m'
  yellow='\033[1;33m'
  blue='\033[1;34m'
  magenta='\033[1;35m'
else
  # Make them all empty (no formatting) if stderr is piped.
  reset=''
  bold=''
  yellow=''
  blue=''
  magenta=''
fi

echo >&2 -e "${bold}Pushed$reset ${blue}${domain}${reset}:${yellow}${service}${reset}@${magenta}${version}$reset"
