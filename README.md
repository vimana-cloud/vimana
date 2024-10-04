# Actio Mono-repo

For an introduction to how Actio works,
see the [internal overview](docs/internal-overview.md).

For general information about documentation, see [`docs`](docs/).

## Developer Setup

1. Clone the [repository](https://github.com/actio-cloud/actio):
   ```bash
   git clone git@github.com:actio-cloud/actio.git
   ```
2. Install [Bazel](https://bazel.build/).

## Commands To Know

Run a local documentation server:

```bash
bazel run //docs:dev
```

Check for updates to any Bazel or Rust dependency in `MODULE.bazel`,
and apply them in place:

```bash
bazel run //util:update-dependencies
```
