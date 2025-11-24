"""Provide a Bazel build rule to invoke the Vimana Protobuf compiler as a build action."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@rules_proto//proto:defs.bzl", "ProtoInfo")

VimanaMetadataInfo = provider(
    "Serialized metadata output from the Vimana Protobuf compiler plugin.",
    fields = {
        "file": "File object containing the serialized Vimana metadata.",
    },
)

WitFileInfo = provider(
    "WIT file output from the Vimana Protobuf compiler plugin.",
    fields = {
        "file": "File object containing the WIT interface.",
    },
)

def _vimana_protoc_impl(ctx):
    parameters = []
    if ctx.attr.ignore_unsupported_features:
        parameters.append("--vimana_opt=ignore-groups,ignore-required")

    proto_info = ctx.attr.proto[ProtoInfo]

    wit_file = ctx.actions.declare_file(paths.join(ctx.label.name, "server.wit"))
    metadata_file = ctx.actions.declare_file("metadata.binpb", sibling = wit_file)

    ctx.actions.run(
        executable = ctx.executable._protoc_bin,
        inputs = proto_info.transitive_sources.to_list(),
        outputs = [wit_file, metadata_file],
        arguments = [
            "--plugin={}".format(ctx.executable._protoc_gen_vimana_bin.path),
            "--vimana_out={}".format(wit_file.dirname),
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
        DefaultInfo(files = depset([wit_file, metadata_file])),
        WitFileInfo(file = wit_file),
        VimanaMetadataInfo(file = metadata_file),
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
        "ignore_unsupported_features": attr.bool(
            doc = "Rather than failing with an error for unsupported field types," +
                  " like groups or required fields," +
                  " simply display a warning and ignore those fields instead." +
                  " Useful for running the Protobuf conformance tests.",
            default = False,
        ),
        "_protoc_bin": attr.label(
            default = "@protobuf//:protoc",
            executable = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "_protoc_gen_vimana_bin": attr.label(
            default = ":compiler",
            executable = True,
            cfg = "exec",
            allow_single_file = True,
        ),
    },
    provides = [VimanaMetadataInfo, WitFileInfo],
)
