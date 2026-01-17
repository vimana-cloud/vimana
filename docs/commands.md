# Commands

Run a local documentation server.
This renders all the Markdown files in the repo using [VitePress].
It also renders the [Mermaid] diagrams embedded in the Markdown.

```bash
bazel run //docs:dev
```

Check for upgrades to any Bazel, Rust, Python, or Go dependencies
and apply them in-place:

```bash
bazel run //dev:upgrade-dependencies
```

Start a local [kind] cluster
using the latest local builds of the runtime and operator:

```bash
bazel run //dev/kind:restart
```

Hot-reload the latest local build of the runtime and operator
into an already-running kind cluster.
This can significantly improve iteration speed when testing locally,
but be mindful of the note at the [top of the script].

```bash
bazel run //dev/kind:hotswap
```

[VitePress]: https://vitepress.dev/
[Mermaid]: https://mermaid.js.org/
[kind]: https://kind.sigs.k8s.io/
[top of the script]: /dev/kind/hotswap.sh
