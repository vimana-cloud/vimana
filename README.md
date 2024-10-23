# Vimana Monorepo

For an introduction to how Vimana works,
see the [internal overview](docs/internal-overview.md).

For general information about documentation, see [`docs`](docs/).

## Developer Setup

1. Clone the [repository](https://github.com/vimana-cloud/vimana):
   ```bash
   git clone git@github.com:vimana-cloud/vimana.git
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
bazel run //dev:update-dependencies
```
