#!/usr/bin/env bash

# Run all the commands in `tutorial.md` to verify that the tutorial works.

set -euo pipefail

# 2. Compile an Implementation

mkdir tmp
function cleanup-tmp {
  rm -r tmp
}
trap cleanup-tmp EXIT

bazel build compiler
bazel run @protobuf//:protoc -- \
  --plugin="$(bazel info bazel-bin)/compiler/protoc-gen-vimana" \
  --vimana_out="$(pwd)/tmp" \
  --proto_path="$(pwd)" \
  cluster/tests/components/mvp.proto

mkdir -p tmp/cluster/tests/components
wit-bindgen c \
  "$(pwd)/tmp/wit" \
  --world=server \
  --out-dir="$(pwd)/tmp/cluster/tests/components"

bazel run @rules_wasm//:wasi-clang -- \
  -mexec-model=reactor \
  -I "$(pwd)/tmp" \
  -o "$(pwd)/tmp/module.wasm" \
  "$(pwd)/tmp/cluster/tests/components/server.c" \
  "$(pwd)/tmp/cluster/tests/components/server_component_type.o" \
  cluster/tests/components/mvp.c

wasm-tools component new \
  "$(pwd)/tmp/module.wasm" \
  --adapt="$(bazel cquery --output=files @rules_wasm//:wasi-snapshot-preview1-reactor)" \
  --output="$(pwd)/tmp/component.wasm"

# 3. Push the Image to a Registry

bazel run //cluster/bootstrap:push-image -- \
  --repository=http://localhost:5000/hello-world-example \
  --version=1.0.0 \
  --component="$(pwd)/tmp/component.wasm" \
  --metadata="$(pwd)/tmp/metadata.binpb"

# 4. Fire Up Kind

bazel run //dev/kind:restart

cloud-provider-kind &
tunnel_pid=$!
function cleanup-tunnel {
  kill $tunnel_pid || true
  cleanup-tmp
}
trap cleanup-tunnel EXIT

# 5. Generate TLS Credentials

bazel run //cluster/bootstrap:self-signed-tls -- \
  "$(pwd)/tmp/self-signed-root.key" \
  "$(pwd)/tmp/self-signed-root.cert" \
  "$(pwd)/tmp/self-signed-certificates.json" \
  00000000000000000000000000000001.app.vimana.host \
  mvp.test

kubectl apply -f "$(pwd)/tmp/self-signed-certificates.json"
function cleanup-certificates {
  kubectl delete -f "$(pwd)/tmp/self-signed-certificates.json" || true
  cleanup-tunnel
}
trap cleanup-certificates EXIT

# 6. Run the Service

bazel build //cluster/tests:mvp.yaml
kubectl apply -f "$(bazel cquery --output=files //cluster/tests:mvp.yaml)"
function cleanup-resources {
  kubectl delete -f "$(bazel cquery --output=files //cluster/tests:mvp.yaml)" || true
  cleanup-certificates
}
trap cleanup-resources EXIT

# 7. Set up DNS

# Wait for the gateway to become available.
sleep 15s

gateway_address="$(
  kubectl get service the-vimana-gateway \
    --output=jsonpath='{.status.loadBalancer.ingress[0].ip}'
)"
echo "${gateway_address} mvp.test" | sudo tee -a /etc/hosts > /dev/null
function cleanup-hosts {
  # Use the backup file trick to make this portable across Linux and MacOS.
  sudo sed -i.backup '/mvp\.test/d' /etc/hosts && sudo rm /etc/hosts.backup || true
  cleanup-resources
}
trap cleanup-hosts EXIT

# 8. A Traditional Greeting

grpcurl \
  -insecure \
  -proto cluster/tests/components/mvp.proto \
  -d '{"name": "World"}' \
  mvp.test:443 \
  foo.ThisOldTrope/HelloWorld
