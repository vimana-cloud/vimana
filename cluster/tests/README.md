# End-to-end Tests

Run tests with access to a Vimana cluster.

## Kind

Use [kind] to test locally.

Start a local kind cluster with Vimana enabled.
Note that this does more than just `kind create cluster`;
it first builds a "node" image with the latest local build of Vimana's container runtime,
installs the Vimana API controller and Envoy Gateway,
and configures various settings.

```bash
bazel run //dev/kind:restart
```

Starting kind can take a minute.
Iterate faster by hot-swapping a freshly-built runtime binary and controller
into the running kind cluster.

```bash
bazel run //dev/kind:hotswap
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

[kind]: https://kind.sigs.k8s.io/
