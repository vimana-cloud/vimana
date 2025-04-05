# Work Node Runtime

The work runtime implements the K8s [Container Runtime Interface] (CRI)
to coordinate running containers with the control plane,
while also listening on UDP port 443 (the data plane)
for gRPC traffic over HTTP/3,
and routing those incoming requests to the running containers.

[Container Runtime Interface]: https://kubernetes.io/docs/concepts/architecture/cri/

## Control Plane

Communication with the control plane happens via the CRI.

## Data Plane

The data plane server is a customized gRPC server implementation
that can serve arbitrarily many heterogeneous services
on a single port.

## State

The runtime 

Work nodes keep track of running containers
in a single global in-memory structure called the pod pool
(this runtime maintains a 1-to-1 relationship between containers and pods,
which is not necessarily the case for other K8s container runtimes).

### Resource Heirarchy

Vimana's resources can be conceptualized in a heirarchy.

Each container runs a single [component](/docs/glossary.md#component),
but there may be several containers running the same component on a single node.
This leads to two essential ways
the runtime may have to identify a component or individual container:

1. By component name (e.g. `example.com:foo.Bar@1.2.3`).
   This happens when a container is created by the control plane
   (because it most load the component)
   and also when traffic comes in from the data plane
   (because it must identify a pod with the correct component
   to serve that traffic).
2. By pod sandbox ID or container ID (e.g. `example.com:foo.Bar@1.2.3#0`).
   Since Vimana maintains a 1-to-1 relationship between pods and containers,
   these two types of IDs are interchangeable in practice.
   However, the Kubelet will treat them as though they are distinct.
   This kind of identification is only used in the control plane.

To support both kinds of lookups,
the work runtime maintains a single global in-memory map
mapping component names to sets of running containers,
and an "ID map" mapping pod sandbox / container IDs to component names.
Data plane traffic is more performance-sensitive than control plane traffic,
so it makes sense to primarily optimize the component map for reading.
Control plane traffic, on the other hand,
must make do with a 2-layer lookup in order to actual find an individual container,
first looking up the 

### Kubectl commands

```bash
kubectl attach example.com:bar.FooService@1.2.3
```
