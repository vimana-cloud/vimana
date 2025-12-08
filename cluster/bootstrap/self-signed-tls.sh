#!/usr/bin/env bash

# Generate root CA credentials
# in the form of PEM-encoded private key / certificate files,
# as well as TLS credentials for any number of domain names
# in the form of a single JSON file containing a K8s `Secret` resource per domain.

# Path to the `tls-generate` tool from `rules_k8s`.
tls_generate="$1"
# Path to the `openssl` binary.
openssl="$2"
# Output path for the root certificate private key.
root_key="$3"
# Output path for the root certificate.
root_cert="$4"
# Output path for the generated K8s `Secret` resources (JSON file).
resources="$5"
shift 5

"$tls_generate" --ca --key="$root_key" --cert="$root_cert" --openssl="$openssl"

key="$(mktemp)"
cert="$(mktemp)"
for domain in "$@"
do
  "$tls_generate" "$domain" \
    --root-key="$root_key" --root-cert="$root_cert" \
    --key="$key" --cert="$cert" \
    --openssl="$openssl"
  cat >> "$resources" <<EOF
{
  "kind": "Secret",
  "apiVersion": "v1",
  "metadata": {
    "name": "c-$(echo -n "$domain" | sha224sum | head -c 56)"
  },
  "type": "kubernetes.io/tls",
  "data": {
    "tls.crt": "$(< "$cert" base64 -w 0)",
    "tls.key": "$(< "$key" base64 -w 0)"
  }
}
EOF
done
rm "$key" "$cert"
