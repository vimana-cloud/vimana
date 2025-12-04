# Protobuf Plugin

A Protobuf service definition is used to generate:

- A WIT interface,
  which the user can use to implement a service.
- Component metadata,
  which the platform uses to host the implementation.

The Vimana [plugin for `protoc`] handles these conversions.

The generated WIT interface is used to compile a Wasm component
that can then be combined with the generated metadata
to produce a Vimana image,
which is a specialized OCI artifact similar to [Wasm OCI artifacts].

Vimana images can be distributed via any OCI-compliant image registry.
A Vimana image provides the implementation of a [Component resource],
which is consumed by the [runtime] to run services.

[plugin for `protoc`]: https://protobuf.dev/reference/other/#plugins
[Wasm OCI artifacts]: https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/
[Component resource]: /operator/
[runtime]: /runtime/

## Standalone Use

To use the plugin with a [local installation of `protoc`],
it's easiest to first install it somewhere on the system `PATH`:

```bash
bazel build --config=release compiler
install --target-directory=/usr/local/bin \
  "$(bazel cquery --config=release --output=files compiler)"
```

Then the plugin can be used by passing the `--vimana_out` flag to `protoc`:

```bash
protoc [OPTIONS] --vimana_out=DIRECTORY [FILES]
```

The output directory will be populated with a subdirectory called `wit/`
and a file called `metadata.binpb`.
The subdirectory is a self-contained WIT package
that can be used to [compile a component] with any toolchain that supports components.

A built component can be combined with the accompanying metadata file
to produce a Vimana image,
which can be pushed to an OCI image registry for use in a Vimana cluster.
The easiest way to do so manually is with the [`push-image` tool].
For details, see:

```bash
bazel run //cluster/bootstrap:push-image -- --help
```

[local installation of `protoc`]: https://protobuf.dev/installation/
[compile a component]: https://component-model.bytecodealliance.org/language-support.html
[`push-image` tool]: /cluster/bootstrap/

## Bazel Use

Projects using Bazel can use the `vimana_protoc` build rule,
which consumes the output of a [`proto_library`] rule,
and produces output that can be used
as the WIT package for any of the component-building rules from [`rules_wasm`],
as well as a metadata source for the [`vimana_image_push`] executable rule.

[`proto_library`]: https://bazel.build/reference/be/protocol-buffer#proto_library
[`rules_wasm`]: https://github.com/vimana-cloud/rules_wasm
[`vimana_image_push`]: /cluster/bootstrap/

## Compiler Limitations

- The compiler does not yet support [Protobuf editions].
- Protobuf packages cannot contain a period,
  so e.g. `package foo;` works while `package foo.bar;` does not.
  This is a temporary limitation
  blocked on support for [nested package namespaces] in WIT.
  * Message definitions cannot be nested within other message definitions.
    This is also a temporary limitation
    that should disappear with the support of nested package names in WIT.
- Vimana does not support some long-deprecated Proto2-only features,
  including [required fields] and [groups].

[Protobuf editions]: https://protobuf.dev/programming-guides/editions/
[nested package namespaces]: https://github.com/bytecodealliance/wasm-tools/issues/2393
[required fields]: https://protobuf.dev/programming-guides/proto2/#field-labels
[groups]: https://protobuf.dev/programming-guides/proto2/#groups
