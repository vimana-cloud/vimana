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

Vimana does *not* bundle a dedicated server stack into each unit of isolation.
Instead, the runtime is responsible for all essential server boilerplate,
including a single, shared gRPC / Protobuf stack.

Instead of binding to a port, maintaining a thread pool,
or worrying about network protocols or message encodings,
each component simply exposes it's API as a set of richly-typed functions,
and the runtime handles the rest.
Transparent green-threading is backed by a single system thread pool.
Cheap sandboxing is implemented in userspace.
Optional JSON transcoding occurs at the gateway.

This can significantly reduce the memory footprint of each "container" image,
especially for FaaS-style use-cases.

## Get Started

Make your hands dirty with the [tutorial].

[tutorial]: docs/tutorial.md
