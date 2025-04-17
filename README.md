# Vimana Monorepo

For an introduction to how Vimana works,
see the [internal overview](docs/internal-overview.md).

For general information about documentation, see [docs](docs/).

## One-Time Setup

1. Clone this repository.
2. (Mac only) Install [core utilities] and [Xcode].
   Make sure you have your [developer permission].
3. Install [Bazelisk].
4. To run integration tests:
   1. Install [Docker](https://docs.docker.com/) and enable the daemon.
   2. Run the container registry [reference implementation](https://hub.docker.com/_/registry)
      with automatic restart forever:
      ```bash
      docker run --detach --restart=always --name=registry --publish=5000:5000 registry:latest
      ```
5. (Optional) Install [`direnv`]
   to automatically set up convenient [aliases to pre-built binaries](.bin/) &mdash;
   like `kubectl` and `wasmtime` &mdash;
   whenever you enter the repository directory in your shell.

[core utilities]: https://formulae.brew.sh/formula/coreutils
[Xcode]: https://apps.apple.com/app/xcode/
[developer permission]: https://developer.apple.com/register/
[Bazelisk]: https://github.com/bazelbuild/bazelisk
[Docker]: https://docs.docker.com/
[reference implementation]: https://hub.docker.com/_/registry
[`direnv`]: https://direnv.net/

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
