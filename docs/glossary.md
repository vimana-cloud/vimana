# Glossary

Naming is one of the only hard problems in computer science.
The following words have precise meaning(s):

#### Alias Domain
1. Any externally-controlled DNS domain
   with either an alias-mode `HTTPS` or `SVCB` record, or `CNAME` record,
   that points to a [canonical domain](#canonical-domain).
   The platform does not control these domains,
   but can provision TLS certificates for them.

##### Canonical Domain
1. A platform-controlled DNS domain
   of the form `<uuid>.app.vimana.host`.

##### Cluster
1. An [Envoy `Cluster`] comprising
   a set of [work nodes](#work-node)
   capable of serving a particular [service version](#version).
   A single work node may be able to serve many versions of many services,
   and so can be included in many different such clusters.
2. A [K8s cluster] orchestrating a Vimana [zone](#zone).

##### Component<a id="version"></a><a id="implementation"></a>
1. A concrete version of a [service](#service),
   defined by an exact service Proto definition
   and a Wasm component that implements it.
   Identified by a [service name](#service) plus a version string.
   Corresponds to an [Envoy `Cluster`].
   - *aka* **version**, **implementation**
   - *eg* `example.com:foo.Bar@1.2.3`

##### Control plane
1. All HTTP / gRPC traffic that is *not* [data plane](#data-plane).
   Generally, anything serving a K8s or Envoy API.

##### Data plane
1. All HTTP / gRPC traffic
   destined to be served by a user component on a [work node](#work-node).

##### Domain
1. Identifies a customer organization.
   Corresponds to a DNS domain.
   - *eg* `example.com`

##### Ingress node
1. A [k8s node] running Envoy
   that receives downstream client traffic
   and forwards it to an appropriate [work node](#work-node).

##### Provider
1. An organization providing infrastructure.
   A datacenter owner.
   Real providers, like `aws`, `gcp`, and `azure`,
   can be used to identify both [zones](#zone) and [regions](#region).
   Composite providers, like `all`,
   can only identify regions,
   being composed of zones from real providers.

##### Region
1. A geographically constrained group of [zones](#zone)
   within a real or composite [provider](#provider).
   Corresponds to the `region` of an [Envoy `Locality`]
   - *eg* `all/eu`

##### Service
1. A named RPC service, defined by a service Proto definition
   that may evolve over time but should adhere to [best practices]
   and strive for backwards compatibility.
   Identified by a [domain](#domain) and a full service name.
   May have an arbitrary number of [versions](#version).
   - *eg* `example.com:foo.Bar`,
     `com.example.Bar` (domain assumed to be reversed package name)

##### Work node
1. A [K8s node] responsible for running service [implementations](#version).

##### Zone
1. An independent [K8s cluster] running deployed [services](#service)
   at the finest grain of geographic specificity.
   Corresponds to a [provider's](#provider) "zone"
   (i.e. an AWS *zone* or GCP *zone*),
   or some complete datacenter in which Vimana runs.
   Corresponds to the `zone` of an [Envoy `Locality`]

[best practices]: https://protobuf.dev/programming-guides/dos-donts/
[envoy `cluster`]: https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/cluster/v3/cluster.proto#config-cluster-v3-cluster
[envoy `locality`]: https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/core/v3/base.proto#config-core-v3-locality
[k8s cluster]: https://kubernetes.io/docs/concepts/architecture/
[k8s node]: https://kubernetes.io/docs/concepts/architecture/nodes/
