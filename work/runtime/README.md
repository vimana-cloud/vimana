# Work Node Runtime

The runtime is the meat of a work node.
It implements the K8s [Container Runtime Interface] (CRI)
to coordinate running containers with the control plane,
while also listening on UDP port 443 (the data plane)
for gRPC traffic over HTTP/3,
and routing those incoming requests to the running containers.

[Container Runtime Interface]: https://kubernetes.io/docs/concepts/architecture/cri/

## Control Plane

Communication with the control plane happens via the CRI.

### Traffic Patterns

<!-- TODO: These are still conjecture. Confirm with e2e tests. -->

#### Container Lifecycle

```
┌──────────────┐                                                         ┌─────┐
│ Work Runtime │                                                         │ K8s │
└┬─────────────┘                                                         └────┬┘
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ RunPodSandbox(metadata) ┥
 ├ Ok(pod-sandbox-id) ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> │
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ CreateContainer(pod-sandbox-id, image-id) ┥
 ├ Ok(container-id) ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> │
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ StartContainer(container-id) ┥
 ├ Ok ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> │
 │                                                                            │
 │                          Container is running...                           │
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ StopContainer(container-id) ┥
 ├ Ok ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> │
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ RemoveContainer(container-id) ┥
 ├ Ok ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> │
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ StopPodSandbox(pod-sandbox-id) ┥
 ├ Ok ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> │
 │                                                                            │
 │ <━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ RemovePodSandbox(pod-sandbox-id) ┥
 └ Ok ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄> ┘
```

## Data Plane

The data plane server is a customized gRPC server implementation
that can serve arbitrarily many heterogeneous services
on a single port.