# Vimana

[![Unit tests status](https://github.com/vimana-cloud/vimana/actions/workflows/unit-tests.yaml/badge.svg)](https://github.com/vimana-cloud/vimana/actions/workflows/unit-tests.yaml)

Vimana is an experimental "container" runtime and Kubernetes API
for running modern web services
built from extremely lightweight [WebAssembly components].

> [!NOTE]
> This project is a **work in progress**.
> It is not ready for serious use in a production environment,
> and all features should be considered unstable.

[WebAssembly components]: https://component-model.bytecodealliance.org/

## Running Services

Vimana is all about running *services*.
Those could be [gRPC] services, HTTP/JSON services, JSON-RPC services,
or potentially other types of services that can be [defined in Protobuf].

Kubernetes has long facilitated running [services] at scale,
but Vimana leverages Wasm to provide a higher level of abstraction,
more advanced built-in features,
and a far more efficient runtime for FaaS-style use-cases.

Vimana consists of 3 principle parts:

1. The [Protobuf compiler plugin],
   which converts Protobuf service definitions to WIT interfaces
   so that server implementations can be compiled as Wasm components.
2. The ["container" runtime], which runs the compiled components as K8s pods.
3. The [K8s operator] providing an ergonomic API
   to spin up and manage Vimana services.

[gRPC]: https://grpc.io/
[defined in Protobuf]: https://protobuf.dev/programming-guides/proto3/#services
[services]: https://kubernetes.io/docs/concepts/services-networking/service/
[Protobuf compiler plugin]: /compiler/
["container" runtime]: /runtime/
[K8s operator]: /operator/

### A New Image

Vimana does not bundle a dedicated server stack into each unit of isolation.
Instead, the runtime is responsible for all essential server boilerplate,
including a single, shared gRPC / Protobuf stack.

Instead of binding to a port, maintaining a thread pool,
or worrying about network protocols or message encodings,
each component simply exposes it's API as a set of richly-typed functions,
and the runtime handles the rest.
Transparent green-threading is backed by a single system thread pool.
Cheap sandboxing is implemented in userspace.

This can significantly reduce the size of each "container" image,
especially for FaaS-style use-cases.

## Hello World

This tutorial walks you through deploying a simple Vimana service.
Before starting, read the [Developer Setup].

[Developer Setup]: /docs/developer-setup.md

### 1. Define a Service

The first step is to define your service(s) in Protobuf.
A classic example is readily available at [`cluster/tests/components/helloworld.proto`]:

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

[`cluster/tests/components/helloworld.proto`]: /cluster/tests/components/helloworld.proto

### 2. Compile an Implementation

The easiest way to compile a component that implements our service
is to just use the implementation that comes with this repo:

```bash
bazel build //cluster/tests/components:helloworld
```

If you run that, you can skip the rest of this section.

But that would be super basic!
Instead, let's walk through the process manually, for fun.

It starts by compiling the Protobuf service definition
into a WIT package and metadata file, using the compiler:

```bash
# Build a fresh copy of `protoc-gen-vimana` from source.
bazel build compiler
# Create a temporary directory to hold demo-related files.
mkdir tmp
# Build and run `protoc` from source, with the fresh build of the plugin.
# See the compiler documentation for more ways to use the compiler.
bazel run @protobuf//:protoc -- \
  --plugin="$(bazel info bazel-bin)/compiler/protoc-gen-vimana" \
  --vimana_out="$(pwd)/tmp" \
  --proto_path="$(pwd)" \
  cluster/tests/components/helloworld.proto
```

See also the [compiler documentation] for more ways to run `protoc` with the Vimana plugin.

The above commands will produce a WIT package at `tmp/wit`.
There are many ways to compile a component against this WIT package [in various languages].
It's 2025, so let's do it in C!
Generate C-native "bindings" for the WIT interface using [`wit-bindgen`]:

```bash
# Mirror the directory structure of the test component source code,
# so the `#include` directive will work as-is.
mkdir -p tmp/cluster/tests/components

# This step produces C-specific "bindings" to the language-agnostic WIT interface,
# including a header file with type definitions.
bazel run @rules_wasm//:wit-bindgen -- \
  c "$(pwd)/tmp/wit" \
  --world=server \
  --out-dir="$(pwd)/tmp/cluster/tests/components"
```

Now, we need an implementation.
The follow example can be found at [`cluster/tests/components/helloworld.c`]:

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
    char * message = (char *)malloc(request->name.len + 9);
    sprintf(message, "Hello, %s!", request->name.ptr);
    server_string_set(&response->message, message);
}
```

Compile that using the [WASI SDK]:

```bash
bazel run @rules_wasm//:wasi-clang -- \
  -mexec-model=reactor \
  -I "$(pwd)/tmp" \
  -o "$(pwd)/tmp/module.wasm" \
  "$(pwd)/tmp/cluster/tests/components/server.c" \
  "$(pwd)/tmp/cluster/tests/components/server_component_type.o" \
  cluster/tests/components/helloworld.c
```

That produces a core module,
which we can turn into a component using [`wasm-tools`]:

```bash
bazel run @rules_wasm//:wasm-tools -- \
  component new \
  "$(pwd)/tmp/module.wasm" \
  --adapt="$(bazel cquery --output=files @rules_wasm//:wasi-snapshot-preview1-reactor)" \
  --output="$(pwd)/tmp/component.wasm"
```

[compiler documentation]: /compiler/
[in various languages]: https://component-model.bytecodealliance.org/building-a-simple-component.html
[`wit-bindgen`]: https://github.com/bytecodealliance/wit-bindgen
[`cluster/tests/components/helloworld.c`]: cluster/tests/components/helloworld.c
[WASI SDK]: https://github.com/WebAssembly/wasi-sdk
[`wasm-tools`]: github.com/bytecodealliance/wasm-tools

### 3. Push an Image to a Registry

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

### 4. Fire Up Minikube

Start a local [minikube] cluster
using the latest local builds of the runtime and operator:

```bash
bazel run //dev/minikube:restart
```

Once the cluster is up, you'll need a tunnel to communicate with it.
This command should probably be running in the background
the whole time the cluster is running.
Note that the `minikube` command is automatically available
if you enabled `direnv` during setup.

```bash
minikube tunnel
```

For a minimal example using the running Vimana cluster,
see [`e2e/mvp.yaml`] and [`e2e/mvp.py`].

```bash
bazel test //e2e:mvp-test
```

[minikube]: https://minikube.sigs.k8s.io/
[`e2e/mvp.yaml`]: e2e/mvp.yaml
[`e2e/mvp.py`]: e2e/mvp.py
