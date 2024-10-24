# Vimana Monorepo

For an introduction to how Vimana works,
see the [internal overview](docs/internal-overview.md).

For general information about documentation, see [`docs`](docs/).

## Developer Setup

1. Clone this repository.
2. Install [Bazelisk](https://github.com/bazelbuild/bazelisk).

## Commands To Know

Run a local documentation server:

```bash
bazel run //docs:dev
```

Check for updates to any Bazel or Rust dependency in `MODULE.bazel`,
and apply them in place:

```bash
bazel run //dev:update-dependencies
```
