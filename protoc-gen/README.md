# Protobuf Plugins

A Protobuf service definition is used to generate:

- a WIT interface,
  which the user can use to implement a service.
- a [version configuration](TODO),
  which the platform uses to host the implementation.

Plugins for `protoc` to handle each use-case
can be found in [`wit/`](wit/) and [`actio/`](actio/), respectively.