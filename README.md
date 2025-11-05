# Vimana Monorepo

[![Unit tests status](https://github.com/vimana-cloud/vimana/actions/workflows/unit-tests.yaml/badge.svg)](https://github.com/vimana-cloud/vimana/actions/workflows/unit-tests.yaml)

Vimana is an experimental "container" runtime and Kubernetes API
for running modern web services
built from extremely lightweight [WebAssembly components].

This project is a **work in progress**.
It is not yet ready for serious use in a production environment.

See the [overview] to learn more.

[WebAssembly components]: https://component-model.bytecodealliance.org/
[overview]: docs/overview.md

## One-Time Setup

1. Clone this repository.
2. (Mac only) Install [core utilities] and [Xcode].
   Make sure you have your [developer permission].
3. Install [Bazelisk].
4. Install [Docker] and enable the daemon.
   1. A container registry is required to run Vimana locally.
      Just run the [reference implementation] with automatic restart forever:
      ```bash
      docker run --detach --restart=always --name=registry --publish=5000:5000 registry:latest
      ```

[core utilities]: https://formulae.brew.sh/formula/coreutils
[Xcode]: https://apps.apple.com/app/xcode/
[developer permission]: https://developer.apple.com/register/
[Bazelisk]: https://github.com/bazelbuild/bazelisk
[Docker]: https://docs.docker.com/
[reference implementation]: https://hub.docker.com/_/registry

## Cluster Provision

### Local

Start a local [minikube] cluster
using the latest local builds of the runtime and operator:

```bash
bazel run //dev/minikube:restart
```

Once the cluster is up, you'll need a tunnel to communicate with it.
This command should probably be running in the background
the whole time the cluster is running.

```bash
minikube tunnel
```

For a minimal example using the running Vimana cluster,
see [`e2e/mvp.yaml`] and [`e2e/mvp.py`].

```bash
bazel test //e2e:mvp-test
```

[minikube]: https://minikube.sigs.k8s.io/
[`e2e/mvp.yaml`]: e2e/mvp.yaml
[`e2e/mvp.py`]: e2e/mvp.py

### Cloud

Vimana aims to make provisioning clusters on various cloud providers as easy as possible,
but currently, only GCP is supported.

To use the GCP backend,
first ensure you have [application default credentials] available on your machine.
The simplest way to do this for a normal Google account is to run:

```bash
gcloud auth application-default login
```

#### Node Image

The first step is to build a node image
with the latest local build of the runtime.
If you own a project with ID `my-project-id`, you can run this:

```bash
bazel run //cluster/node:make-image -- --gcp-project="my-project-id"
```

That script will spin up a temporary GCE instance to build the node image,
then shut the instance down once the image is ready.
The whole process should take about five minutes.

#### Cluster

Profiles provide a convenient way
to keep track of the private details related to cluster management.

If you haven't yet, edit [`cluster/profiles/profiles.yaml`],
replacing `gcp-example-with-custom-node-image.com` with a new name,
*e.g.* `my-cluster.net`
(it *does not* have to be a real domain).
Edit the following fields:

- `state-store` should identify a usable [kOps state store].
  This can be the URI of a Google Storage bucket that you own.
- `project` is the ID of the project that will own the cluster.
  This may or may not be the same as `image-project`.
- `image-project` should be the same project you used to make the node image
  (`my-project-id` in the example above).
- `image-family` should be either `vimana` or `vimana-dirty`,
  depending on whether the node image was created from a clean Git worktree
  (the node image creation script will tell you which to use).
  The cluster will use the latest image within this family.

Once the profile is configured, use it to create your cluster:

```bash
bazel run //cluster:create -- 'my-cluster.net' # or whatever you named it
```

You can interact with the new cluster using `kubectl`.
Once you're done with it:

```bash
bazel run //cluster:destroy -- 'my-cluster.net'
```

[application default credentials]: https://cloud.google.com/docs/authentication/application-default-credentials
[`cluster/profiles/profiles.yaml`]: cluster/profiles/profiles.yaml
[kOps state store]: https://kops.sigs.k8s.io/state/

## Tools

Most of the major tools you need to work with Vimana
are automatically sourced from official GitHub release binaries.
Just install [`direnv`] to set up convenient [tool aliases]
whenever you enter the repository directory in your shell.

The following tool aliases are provided:

- `bazel-docker`
- `crane`
- `crictl`
- `istioctl`
- `kops`
- `kubectl`
- `kustomize`
- `minikube` (only on x86-64)
- `openssl`
- `sudo-persist`
- `wasmtime`
- `wasm-tools`

[`direnv`]: https://direnv.net/
[tool aliases]: dev/tools/

### Bazel Container

Vimana builds fine on any Linux system.
However, it relies on some Linux-specific features
that make building or testing certain things directly on a Mac impractical:

- The [runtime] uses [`rtnetlink`], which cannot be built natively for Mac.
  The runtime can always be cross-compiled for Linux
  (which is always the case when building node images)
  but it cannot be tested locally on a Mac.
- The [runtime tests] use Bazel's [`requires-fakeroot`] tag
  (in order to manipulate the network device using `rtnetlink`),
  and that tag is only supported by Bazel on Linux.

To work around this, any Bazel command can be run in a persistent container
dedicated to the current Git worktree.
Use the built-in [`bazel-docker`] script
(which is available automatically after enabling [`direnv`] &mdash; see [tools])
as a drop-in replacement for `bazel`, *e.g.*

```bash
bazel-docker test //runtime/tests/...
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

[runtime]: runtime/
[`rtnetlink`]: https://en.wikipedia.org/wiki/Netlink
[runtime tests]: runtime/tests/
[`requires-fakeroot`]: https://bazel.build/reference/be/common-definitions#common-attributes
[`bazel-docker`]: dev/tools/bazel-docker
[tools]: #tools

### VSCode

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
