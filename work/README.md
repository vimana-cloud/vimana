# Work Nodes

Work nodes receive [data plane] requests from [ingress]
and execute Wasm functions to render responses.

Each is a [K8s node] with a custom [container runtime]
to manage heterogeneous Wasm [components] as [pods].

[data plane]: /glossary.md#data-plane
[ingress]: /ingress/README.md
[k8s node]: https://kubernetes.io/docs/concepts/architecture/nodes/
[container runtime]: runtime/
[components]: /docs/glossary.md#version
[pods]: https://kubernetes.io/docs/concepts/workloads/pods/