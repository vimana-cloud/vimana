"""Provide a Bazel build rule to invoke the Vimana Protobuf compiler as a build action."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@protobuf//bazel/common:proto_info.bzl", "ProtoInfo")
load("@rules_wasm//wasm:defs.bzl", "WitPackageInfo")

PROTOC_TOOLCHAIN = "@protobuf//bazel/private:proto_toolchain_type"

MetadataInfo = provider(
    "Serialized metadata output from the Vimana Protobuf compiler plugin.",
    fields = {
        "file": "File object containing the serialized Vimana metadata.",
    },
)

def _snake_to_kebab_case(snake_case):
    return snake_case.replace("_", "-")

def _proto_to_wit_package_name(proto_name):
    """
    Re-implementation in Starlark
    of the Protobuf-to-WIT package name conversion logic from the compiler.
    """
    return ":".join(
        [_snake_to_kebab_case(part) for part in proto_name.split(".")] +
        ["proto"],
    )

def _vimana_protoc_impl(ctx):
    parameters = []
    if ctx.attr.ignore_unsupported_features:
        parameters.append("--vimana_opt=ignore-groups,ignore-required")
    protoc = ctx.toolchains[PROTOC_TOOLCHAIN].proto.proto_compiler
    proto_info = ctx.attr.proto[ProtoInfo]

    wit_package = ctx.actions.declare_directory(paths.join(ctx.label.name, "wit"))
    metadata_file = ctx.actions.declare_file("metadata.binpb", sibling = wit_package)
    ctx.actions.run(
        executable = protoc,
        inputs = proto_info.transitive_sources.to_list(),
        outputs = [wit_package, metadata_file],
        arguments = [
            "--plugin={}".format(ctx.executable._protoc_gen_vimana_bin.path),
            "--vimana_out={}".format(metadata_file.dirname),
        ] + [
            "--proto_path={}".format(path)
            for path in proto_info.transitive_proto_path.to_list()
        ] + [
            src.path
            for src in proto_info.direct_sources
        ] + parameters,
        tools = [ctx.executable._protoc_gen_vimana_bin],
    )

    return [
        DefaultInfo(files = depset([wit_package, metadata_file])),
        WitPackageInfo(
            package = _proto_to_wit_package_name(ctx.attr.package),
            directory = wit_package,
            # Don't bother listing the WIT dependencies (i.e. `grpc.wit`, WASI).
            # That means the transitive dependencies would be broken
            # if this is used as a dependency of a downstream WIT package,
            # but that's not an intended use-case,
            # so this is considered acceptable for the sake of simplicity.
            # It should always be compiled directly into a component.
            deps = depset(),
        ),
        MetadataInfo(file = metadata_file),
    ]

vimana_protoc = rule(
    implementation = _vimana_protoc_impl,
    doc = "Apply a configuration to resource(s) based on a YAML file.",
    attrs = {
        "proto": attr.label(
            doc = "A proto_library target containing the Protobuf definitions to compile.",
            mandatory = True,
            providers = [ProtoInfo],
        ),
        "package": attr.string(
            doc = "The Protobuf package name of the services being compiled.",
            mandatory = True,
        ),
        "ignore_unsupported_features": attr.bool(
            doc = "Rather than failing with an error for unsupported field types," +
                  " like groups or required fields," +
                  " simply display a warning and ignore those fields instead." +
                  " Useful for running the Protobuf conformance tests.",
            default = False,
        ),
        "_protoc_gen_vimana_bin": attr.label(
            default = ":compiler",
            executable = True,
            cfg = "exec",
            allow_single_file = True,
        ),
    },
    toolchains = [PROTOC_TOOLCHAIN],
    provides = [MetadataInfo, WitPackageInfo],
)

# The only purpose of this rule is to provide a prebuilt `protoc` executable
# for use as a data dependency in the unit tests.
def _prebuilt_protoc_impl(ctx):
    protoc = ctx.toolchains[PROTOC_TOOLCHAIN].proto.proto_compiler

    output = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.symlink(
        target_file = protoc.executable,
        output = output,
        is_executable = True,
    )

    return [DefaultInfo(executable = output)]

prebuilt_protoc = rule(
    executable = True,
    implementation = _prebuilt_protoc_impl,
    doc = "Provide a prebuilt protoc executable.",
    toolchains = [PROTOC_TOOLCHAIN],
)
