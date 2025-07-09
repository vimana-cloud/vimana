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
- `kops`
- `kubectl`
- `kustomize`
- `minikube` (only on x86-64)
- `openssl`
- `wasmtime`
- `wasm-tools`

[`direnv`]: https://direnv.net/
[tool aliases]: .bin/

## Commands To Know

Run a local documentation server.
This renders all the Markdown files in the repo using [VitePress].
It also renders the [Mermaid] diagrams embedded in the Markdown.

```bash
bazel run //docs:dev
```

Start a local [minikube] cluster with Vimana enabled.
This is one way to run the [end-to-end] tests if the local machine is x86-64.
Note that this does more than just `minikube start`;
it first builds a "kicbase" image with the latest local build of `workd`,
and uses a fork of minikube with `workd` enabled.

```bash
bazel run //dev/minikube:restart
```

Starting minikube can take a while.
Iterate faster by hot-swapping a freshly-built runtime binary
into the running minikube cluster
(but read the implications at the top of [`hotswap.sh`] first):

```bash
bazel run //dev/minikube:hotswap
```

Check for updates to any Bazel or Rust dependency in `MODULE.bazel`,
and apply them in-place:

```bash
bazel run //dev:update-dependencies
```

[VitePress]: https://vitepress.dev/
[Mermaid]: https://mermaid.js.org/
[minikube]: https://minikube.sigs.k8s.io/
[end-to-end]: e2e/
[`hotswap.sh`]: dev/minikube/hotswap.sh

### Run Bazel in a Container

Run any build or test inside a container,
simply by using the [`bazel-docker`] command instead of `bazel`.
This command is available automatically after enabling [`direnv`].

This is mainly intended to run certain tests on MacOS,
such as those under [`//work/runtime/tests`],
that use Bazel's [`requires-fakeroot`] tag,
which only works on Linux.

```bash
bazel-docker test //work/runtime/tests/...
```

Containerized Bazel uses a distinct build cache from normal Bazel,
but that cache is shared across invocations.
Note, however, that the analysis cache must be rebuilt on each invocation.

[`bazel-docker`]: .bin/bazel-docker
[`//work/runtime/tests`]: work/runtime/tests
[`requires-fakeroot`]: https://bazel.build/reference/be/common-definitions#common-attributes

## VSCode

The repository includes some VSCode workspace settings:

- **Recommended extensions:**<br />
  VSCode will bug you about them whenever you open the workspace,
  until they are installed.
- **A default build task:**<br />
  Invoke it with `Ctrl+Shift+B` by default.
  This task builds all Bazel rules
  in the same package as the source file that is currently open
  which have a direct dependency on that file.
- **A default test task:**<br />
  VSCode does not provide a keybinding to invoke it by default.
  You can configure one for [`workbench.action.tasks.test`].
  This task runs all Bazel test rules
  which would be built by the default build task,
  or which have a direct dependency on such a rule (in any package).
- A task to automatically generate a `rust-project.json` file based on the Bazel rules
  when the workspace is opened.
  This allows the recommended [rust-analyzer] extension
  to function in a non-Cargo workspace.
- Various formatting rules.

[`workbench.action.tasks.test`]: https://code.visualstudio.com/docs/reference/default-keybindings#_tasks
[rust-analyzer]: https://rust-analyzer.github.io/
