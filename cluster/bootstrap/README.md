# Cluster Bootstrapping Tools

## Pushing Images

A script is provided to push Vimana images
(consisting of a compiled component and matching metadata; see [Compiler])
to an OCI image registry.

To see usage information, run:

```bash
bazel run //cluster/bootstrap:push-image -- --help
```

[Compiler]: /compiler
