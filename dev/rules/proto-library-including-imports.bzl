"""Rule to produce a self-contained proto descriptor set with transitive imports.

Bazel's proto_library only includes direct sources in its descriptor set.
This rule uses protoc --descriptor_set_in and --include_imports to produce
a descriptor set containing all transitively reachable imports,
tree-shaken to only what is needed by the root proto library.
"""

load("@protobuf//bazel/common:proto_info.bzl", "ProtoInfo")

PROTOC_TOOLCHAIN = "@protobuf//bazel/private:proto_toolchain_type"

def _proto_library_including_imports_impl(ctx):
    proto_info = ctx.attr.proto[ProtoInfo]
    descriptor_sets = proto_info.transitive_descriptor_sets.to_list()
    output = ctx.actions.declare_file(ctx.label.name + ".bin")

    # Determine the import paths of the direct sources.
    # These must match the `name` field in each FileDescriptorProto,
    # which is relative to the proto source root.
    source_root = proto_info.proto_source_root
    import_paths = []
    for src in proto_info.direct_sources:
        path = src.path
        if source_root and source_root != "." and path.startswith(source_root + "/"):
            path = path[len(source_root) + 1:]
        import_paths.append(path)

    ctx.actions.run(
        executable = ctx.toolchains[PROTOC_TOOLCHAIN].proto.proto_compiler,
        arguments = [
            "--descriptor_set_in=" + ":".join([ds.path for ds in descriptor_sets]),
            "--descriptor_set_out=" + output.path,
            "--include_imports",
        ] + import_paths,
        inputs = descriptor_sets,
        outputs = [output],
    )

    return [DefaultInfo(files = depset([output]))]

proto_library_including_imports = rule(
    implementation = _proto_library_including_imports_impl,
    attrs = {
        "proto": attr.label(
            providers = [ProtoInfo],
            mandatory = True,
        ),
    },
    toolchains = [PROTOC_TOOLCHAIN],
)
