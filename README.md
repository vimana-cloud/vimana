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
4. Install [Docker] and enable the daemon.
   1. Run the container registry [reference implementation]
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

### Bazel Container

Vimana builds fine on any Linux system.
However, it relies on some Linux-specific features
that make building or testing certain things directly on a Mac impractical:

- The [work runtime] uses [`rtnetlink`], which cannot be built natively for Mac.
  The runtime can always be cross-compiled for Linux
  (which is always the case when building node images)
  but it cannot be tested locally on a Mac.
- The [work runtime tests] use Bazel's [`requires-fakeroot`] tag
  (in order to manipulate the network device using `rtnetlink`),
  and that tag is only supported by Bazel on Linux.

To work around this, any Bazel command can be run in a persistent container
dedicated to the current Git worktree.
Use the built-in [`bazel-docker`] script
(which is available automatically after enabling [`direnv`] &mdash; see [tools])
as a drop-in replacement for `bazel`, *e.g.*

```bash
bazel-docker test //work/runtime/tests/...
```

> [!NOTE]
> In order to work around a subtle issue with bind-mounting MacOS directories in Docker,
> `bazel-docker` transparently manages a persistent secondary container called `bazel-output-sync`
> to synchronize the build cache with the host.
> When that container first starts,
> build artifacts and test logs will only become available on the host system
> after a significant delay (perhaps a few minutes).
> After that initial sync,
> subsequent invocations of `bazel-docker` should only incur modest lag (perhaps a second)
> before output files are available.

[work runtime]: work/runtime
[`rtnetlink`]: https://en.wikipedia.org/wiki/Netlink
[work runtime tests]: work/runtime/tests
[`requires-fakeroot`]: https://bazel.build/reference/be/common-definitions#common-attributes
[`bazel-docker`]: .bin/bazel-docker
[tools]: #tools

### Tools

Most of the major tools you need to work with Vimana
are automatically sourced from official GitHub release binaries.
Just install [`direnv`] to set up convenient [tool aliases]
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

## Commands For You

Run a local documentation server.
This renders all the Markdown files in the repo using [VitePress].
It also renders the [Mermaid] diagrams embedded in the Markdown.

```bash
bazel run //docs:dev
```

Check for updates to any Bazel or Rust dependency in `MODULE.bazel`,
and apply them in-place:

```bash
bazel run //dev:update-dependencies
```

[VitePress]: https://vitepress.dev/
[Mermaid]: https://mermaid.js.org/

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
  This task runs all Bazel test rules (in any package)
  which directly depend on a rule that's built by the default build task,
  or which are themselves included in the default build.
- A task to automatically generate a `rust-project.json` file based on the Bazel rules
  when the workspace is opened.
  This allows the recommended [rust-analyzer] extension
  to function in a non-Cargo workspace.
- Various formatting rules.

[`workbench.action.tasks.test`]: https://code.visualstudio.com/docs/reference/default-keybindings#_tasks
[rust-analyzer]: https://rust-analyzer.github.io/
