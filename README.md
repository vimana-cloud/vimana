# Vimana Monorepo

[![Unit tests status](https://github.com/vimana-cloud/vimana/actions/workflows/unit-tests.yaml/badge.svg)](https://github.com/vimana-cloud/vimana/actions/workflows/unit-tests.yaml)

For an introduction to how Vimana works, see the [internal overview].

For general information about documentation, see [docs].

[internal overview]: docs/internal-overview.md
[docs]: docs/

## One-Time Setup

1. Clone this repository.
2. (Mac only) Install [core utilities] and [Xcode].
   Make sure you have your [developer permission].
3. Install [Bazelisk].
4. To run integration tests:
   1. Install [Docker] and enable the daemon.
   2. Run the container registry [reference implementation]
      with automatic restart forever:
      ```bash
      docker run --detach --restart=always --name=registry --publish=5000:5000 registry:latest
      ```

[core utilities]: https://formulae.brew.sh/formula/coreutils
[Xcode]: https://apps.apple.com/app/xcode/
[developer permission]: https://developer.apple.com/register/
[Bazelisk]: https://github.com/bazelbuild/bazelisk
[Docker]: https://docs.docker.com/
[reference implementation]: https://hub.docker.com/_/registry

### Tools

Most of the major tools you need to work with Vimana are included &mdash;
no installation required.
Just install [`direnv`]
to automatically set up convenient [tool aliases]
whenever you enter the repository directory in your shell.

The following tool aliases are provided:

- `crane`
- `crictl`
- `istioctl`
- `kubectl`
- `kustomize`
- `minikube` (only on x86-64)
- `openssl`
- `wasmtime`
- `wasm-tools`

[`direnv`]: https://direnv.net/
[tool aliases]: .bin/

## Commands To Know

Run a local documentation server:

```bash
bazel run //docs:dev
```

Start a local [minikube](https://minikube.sigs.k8s.io/docs/) cluster with Vimana enabled.
This is one way to run the [end-to-end](e2e/) tests.

```bash
bazel run //dev/minikube:restart
```

Starting minikube can take a while.
Iterate faster by hot-swapping a freshly-built runtime binary
into the running minikube cluster
(but read the implications at the top of [`hotswap.sh`](dev/minikube/hotswap.sh) first):

```bash
bazel run //dev/minikube:hotswap
```

Check for updates to any Bazel or Rust dependency in `MODULE.bazel`,
and apply them in place:

```bash
bazel run //dev:update-dependencies
```
