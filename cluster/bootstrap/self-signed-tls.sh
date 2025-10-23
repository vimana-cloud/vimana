tls_generate="$1"
openssl="$2"
root_key="$3"
root_cert="$4"
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
