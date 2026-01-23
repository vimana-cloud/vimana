# Hello World

This tutorial walks you through deploying a simple Vimana service.

Before starting, read the [Developer Setup].
If you set up `direnv` as described,
all the tools referenced in this tutorial (`wasm-tools`, `kind`, `kubectl`, etc.)
will be available automatically when your working directory is within the repository,
but you can also use local installations of each tool if you prefer.

[Developer Setup]: developer-setup.md

## 1. Define a Service

The first step is to define your service(s) in Protobuf.
A classic example is readily available at [`cluster/tests/components/mvp.proto`]:

```proto
syntax = "proto3";

package foo;

service ThisOldTrope {
  rpc HelloWorld(HelloRequest) returns (HelloResponse) {}
}

message HelloRequest {
  string name = 1;
}

message HelloResponse {
  string message = 1;
}
```

[`cluster/tests/components/mvp.proto`]: /cluster/tests/components/mvp.proto

## 2. Compile an Implementation

The easiest way to compile a component that implements our service
is to just use the implementation that comes with this repo:

```bash
bazel build //cluster/tests/components:mvp
```

If you run that, you can skip the rest of this section.

But that would be super basic!
Instead, let's walk through a build process manually,
for old times' sake.

It starts by compiling the Protobuf service definition
into a [WIT interface] and metadata file, using the compiler:

```bash
# Create a temporary directory to hold demo-related files.
mkdir tmp
# Build a fresh copy of `protoc-gen-vimana` from source.
bazel build compiler
# Build and run `protoc` from source, with the fresh build of the plugin.
bazel run @protobuf//:protoc -- \
  --plugin="$(bazel info bazel-bin)/compiler/protoc-gen-vimana" \
  --vimana_out="$(pwd)/tmp" \
  --proto_path="$(pwd)" \
  cluster/tests/components/mvp.proto
```

See also the [compiler documentation] for more ways to run `protoc` with the Vimana plugin.

The above commands will produce a WIT package at `tmp/wit`.
There are many ways to compile a component against this WIT package [in various languages].
It's 2026, so let's do it in C!
Generate C-native "bindings" for the WIT interface using [`wit-bindgen`]:

```bash
# Mirror the directory structure of the test component source code,
# so the `#include` directive will work as-is.
mkdir -p tmp/cluster/tests/components

# This step produces C-specific "bindings" to the language-agnostic WIT interface,
# including a header file with type definitions.
wit-bindgen c \
  "$(pwd)/tmp/wit" \
  --world=server \
  --out-dir="$(pwd)/tmp/cluster/tests/components"
```

Now, we need an implementation.
The follow example can be found at [`cluster/tests/components/mvp.c`]:

```c
#include <stdlib.h>
#include <stdio.h>

#include "cluster/tests/components/server.h"

void this_old_trope_hello_world(
    this_old_trope_hello_request_t *request,
    this_old_trope_context_t *context,
    this_old_trope_hello_response_t *response
) {
    // "Hello, !" is 9 bytes (including the terminating NULL).
    size_t message_length = request->name.len + 9;
    char * message = (char *)malloc(message_length);
    snprintf(message, message_length, "Hello, %s!", request->name.ptr);
    // This transfers "ownership" of the string, so we don't have to free it.
    server_string_set(&response->message, message);
}
```

Compile that using the [WASI SDK]:

```bash
# An official binary release of `clang` from the WASI SDK is included,
# but a local installation would work as well.
bazel run @rules_wasm//:wasi-clang -- \
  -mexec-model=reactor \
  -I "$(pwd)/tmp" \
  -o "$(pwd)/tmp/module.wasm" \
  "$(pwd)/tmp/cluster/tests/components/server.c" \
  "$(pwd)/tmp/cluster/tests/components/server_component_type.o" \
  cluster/tests/components/mvp.c
```

That produces a core module,
which we can turn into a component using [`wasm-tools`]:

```bash
wasm-tools component new \
  "$(pwd)/tmp/module.wasm" \
  --adapt="$(bazel cquery --output=files @rules_wasm//:wasi-snapshot-preview1-reactor)" \
  --output="$(pwd)/tmp/component.wasm"
```

[WIT interface]: https://component-model.bytecodealliance.org/design/interfaces.html
[compiler documentation]: /compiler/
[in various languages]: https://component-model.bytecodealliance.org/building-a-simple-component.html
[`wit-bindgen`]: https://github.com/bytecodealliance/wit-bindgen
[`cluster/tests/components/mvp.c`]: /cluster/tests/components/mvp.c
[WASI SDK]: https://github.com/WebAssembly/wasi-sdk
[`wasm-tools`]: https://github.com/bytecodealliance/wasm-tools

## 3. Push the Image to a Registry

In order to make our component available to a K8s cluster,
it needs to be packaged as an image and distributed via a registry.
Vimana provides an easy script for that:

```bash
bazel run //cluster/bootstrap:push-image -- \
  --repository=http://localhost:5000/hello-world-example \
  --version=1.0.0 \
  --component="$(pwd)/tmp/component.wasm" \
  --metadata="$(pwd)/tmp/metadata.binpb"
```

The above command assumes you're running a registry locally on port 5000,
as show in the [Developer Setup].

## 4. Fire Up Kind


```bash
bazel run //dev/kind:restart && tput bel && cloud-provider-kind
```

This convenient command chain:

1. Starts a local [kind] cluster
   with the latest local builds of the runtime and operator.
   If an old cluster was already running, it is shut down first.
2. Beeps when the cluster is ready.
3. [Opens a tunnel] to communicate with load balancers in the cluster.
   `cloud-provider-kind` will run until manually killed with `Ctrl+C`.
   Killing it will not shut down the cluster,
   but the cluster will be unreachable from the host unless it is running.

[kind]: https://kind.sigs.k8s.io/
[Opens a tunnel]: https://github.com/kubernetes-sigs/cloud-provider-kind

## 5. Generate TLS Credentials

Vimana currently insists on using TLS at the gateway.
This doesn't have to be scary, though,
because it comes with a script to conveniently manage self-signed certificates
for testing:

```bash
# Generate the root CA credentials
# and TLS credentials for the domain `mvp.test` as a K8s `Secret` object.
bazel run //cluster/bootstrap:self-signed-tls -- \
  "$(pwd)/tmp/self-signed-root.key" \
  "$(pwd)/tmp/self-signed-root.cert" \
  "$(pwd)/tmp/self-signed-certificates.json" \
  00000000000000000000000000000001.app.vimana.host \
  mvp.test

# Upload the domain's TLS credentials to the cluster.
kubectl apply -f "$(pwd)/tmp/self-signed-certificates.json"
```

## 6. Run the Service

For a minimal example using the running Vimana cluster,
see [`cluster/tests/mvp.yaml.tmpl`].
This set of resources makes use of the image we pushed to the local registry earlier
and the TLS credentials we just created.

It's defined as a template
so you can configure the registry from which the image is pulled.
Rendering the template with the default value
makes it usable in the local kind cluster.

```bash
bazel build //cluster/tests:mvp.yaml
kubectl apply -f "$(bazel cquery --output=files //cluster/tests:mvp.yaml)"
```

[`cluster/tests/mvp.yaml.tmpl`]: /cluster/tests/mvp.yaml.tmpl

## 7. Set up DNS

Since we're using TLS,
the request must include the right domain (`mvp.test`).
Routing to this domain requires a local override in `/etc/hosts`.

```bash
# Get the external IP address of the gateway.
# `cloud-provider-kind` must be running for this to work.
gateway_address="$(
  kubectl get service the-vimana-gateway \
    --output=jsonpath='{.status.loadBalancer.ingress[0].ip}'
)"

# Add the line `<ip-address> mvp.test` to the hosts file.
echo "${gateway_address} mvp.test" | sudo tee -a /etc/hosts
```

## 8. A Traditional Greeting

Guess what this says!

```bash
grpcurl \
  -insecure \
  -proto cluster/tests/components/mvp.proto \
  -d '{"name": "World"}' \
  mvp.test:443 \
  foo.ThisOldTrope/HelloWorld
```

## 9. Cleanup

If you're done with this example,
it's probably best to clean up up your hosts file.
This command uses the [backup file trick]
to make it portable across Linux and MacOS:

```bash
sudo sed -i.backup '/mvp\.test/d' /etc/hosts && sudo rm /etc/hosts.backup
```

If you're done with the kind cluster, shut it down:

```bash
kind delete cluster
```

Alternatively, if you want to keep the cluster up,
but clean up the resources we created,
run:

```bash
kubectl delete -f cluster/tests/mvp.yaml
kubectl delete -f "$(pwd)/tmp/self-signed-certificates.json"
```

[backup file trick]: https://stackoverflow.com/a/22084103
