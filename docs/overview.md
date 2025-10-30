# Overview

## Resources

Vimana defines a set of [custom Kubernetes resources]
to define and manage your services.
These resources are organized into a simple hierarchy.

Custom resource definitions can be found under [`operator/config/crd/bases/`].
For simple examples of each resource, see [`mvp.yaml`]

[custom Kubernetes resources]: https://kubernetes.io/docs/concepts/extend-kubernetes/api-extension/custom-resources/
[`operator/config/crd/bases/`]: /operator/config/crd/bases/
[`mvp.yaml`]: /e2e/mvp.yaml

### Vimanas

At the top of the hierarchy is the `Vimana` resource.
Each `Vimana` essentially maps to a K8s [gateway]
that exposes its constituent services to external traffic.

Multiple `Vimana` resources may co-exist within a cluster,
but typically there is only a single `Vimana` per cluster.

[gateway]: https://kubernetes.io/docs/concepts/services-networking/gateway/

### Domains

`Domain` resources exist under the `Vimana`.
They provide the information necessary for [SNI]-based routing,
gRPC server reflection, OpenAPI schema generation,
and other domain-wide configuration.

Each `Domain` is identified by a unique 32-character hexadecimal ID,
which in turn defines a *canonical domain name* of the form
`0123456789abcdef0123456789abcdef.vimana.host`.
In addition the canonical domain name,
each `Domain` may have a number of arbitrary *alias domain names*.
allowing the use of custom domains.

[SNI]: https://en.wikipedia.org/wiki/Server_Name_Indication

### Servers

`Server` resources exist under a `Domain`.
Each `Server` defines a set of one or more gRPC services
that are implemented, deployed, and upgraded as a unit.

A server does not necessarily represent a single machine &mdash;
it *does not* correspond to a K8s pod.
Rather, it defines the properties of a set of services
which do not change across version upgrades,
like authentication or feature flags.

A `Server` is given an arbitrary ID, *e.g.* `my-server`,
that must be unique within its domain.
The server can be uniquely identified as `<domain-id>:<server-id>`,
*e.g.* `0123456789abcdef0123456789abcdef:my-server`.

### Components

`Component` resources exist under a `Server`.
Each component represents a concrete,
versioned implementation of the server.

Multiple components (also referred to as *versions*)
may co-exist at the same time for a given server.
Traffic will be distributed to each version
according to the `versionWeights` field on the `Server` instance.

Each component references an image,
which is a specialization of an OCI container image
that contains a compiled Wasm component
and its associated metadata necessary for the Vimana runtime to function.

Each component must be given a valid [semantic version]
that is unique within the parent server.
The component can be identified as `<domain-id>:<server-id>@<component-version>`,
*e.g.* `0123456789abcdef0123456789abcdef:my-server@1.0.0`.
Version numbers cannot be re-used.

[semantic version]: https://semver.org/
