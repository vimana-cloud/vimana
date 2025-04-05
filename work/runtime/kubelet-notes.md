# Notes on Kubelet Behavior

Some notes on how the [kubelet] operates
to help with debugging and understanding how the work runtime operates.

[kubelet]: https://kubernetes.io/docs/reference/command-line-tools-reference/kubelet/

## PLEG

A kubelet process called the Pod Lifecycle Event Generator (PLEG)
periodically invokes `ListPodSandbox`, `ListContainers`, and `PodSandboxStatus`
to refresh a cache of information about pod and container statuses.
That cache is used to serve API calls like `kubectl get pods`,
so the load on the container runtime is smooth and independent of K8s API traffic.

Links:

1. Generic PLEG [entrypoint](https://github.com/kubernetes/kubernetes/blob/v1.32.3/pkg/kubelet/pleg/generic.go#L157).

## Container Lifecycles

<!-- TODO: These traffic patterns are still conjecture. Confirm with e2e tests. -->

Starting and stopping a container.

```mermaid
sequenceDiagram
    participant kubelet as kubelet
    participant workd as Work Runtime
    participant registry as Image Registry

    kubelet ->> workd: RunPodSandbox
    workd ->> registry: Pull
    activate registry
    workd -->> kubelet: Ok
    kubelet ->> workd: CreateContainer
    workd -->> kubelet: Ok
    kubelet ->> workd: StartContainer
    workd ->> registry: Load
    registry -->> workd: Ok
    deactivate registry
    workd -->> kubelet: Ok
    Note left of workd: Container is running …

    kubelet ->> workd: StopContainer
    workd -->> kubelet: Ok
    kubelet ->> workd: RemoveContainer
    workd -->> kubelet: Ok
    kubelet ->> workd: StopPodSandbox
    workd -->> kubelet: Ok
    kubelet ->> workd: RemovePodSandbox
    workd -->> kubelet: Ok
```
