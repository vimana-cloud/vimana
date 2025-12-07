# End-to-end Tests

Run tests with access to a Vimana cluster.

## Minikube

Use [minikube] to test locally.

Start a local minikube cluster with Vimana enabled.
Note that this does more than just `minikube start`;
it first builds a "kicbase" image with the latest local build of Vimana's container runtime,
uses a fork of minikube that supports that runtime,
and installs the Vimana API controller and Envoy Gateway.

```bash
bazel run //dev/minikube:restart
```

Starting minikube can take a while.
Iterate faster by hot-swapping a freshly-built runtime binary and controller
into the running minikube cluster.

```bash
bazel run //dev/minikube:hotswap
```

> [!IMPORTANT]
> Hot-swapping should not affect any running `kube-system` containers that use containerd,
> however it does forcibly shut down any running Vimana containers
> *without notifying the control plane*, which may cause strange behavior
> including disappeared pods getting replaced by the deployment controller.
>
> You generally don't have to worry about this between E2E test runs,
> since each test uses a unique K8s namespace that is deleted on exit
> (unless cleanup is explicitly disabled).

[minikube]: https://minikube.sigs.k8s.io/
