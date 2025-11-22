"""Provide a Bazel build rule to invoke the Vimana Protobuf compiler as a build action."""

load("@bazel_skylib//lib:paths.bzl", "paths")

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

def _closest_common_ancestor(files):
    """Find the closest common ancestor directory of a list of files."""
    if not files:
        return ""

    directories = [paths.dirname(f.path) for f in files]
    if len(directories) == 1:
        return directories[0]

    directory_parts = [directory.split("/") for directory in directories]
    common_parts = []
    for parts in zip(*directory_parts):
        if len(set(parts)) == 1:
            common_parts.append(parts[0])
        else:
            break

    return "/".join(common_parts) if common_parts else ""

def _vimana_protoc_impl(ctx):
    # The set of all files that may be imported by any of the source files.
    include_files = []

    # The set of import paths that will be passed to `protoc`.
    # This is derived from the closest common ancestor directory of each include file group,
    # after stripping the import path prefix from the end of that directory.
    include_paths = set()

    for filegroup, import_prefix in ctx.attr.include.items():
        files = filegroup.files.to_list()
        include_files.extend(files)

        directory = _closest_common_ancestor(files)

        if import_prefix:
            import_prefix = "/" + import_prefix
            if not directory.endswith(import_prefix):
                fail("Expected path '{}' to end with '{}'".format(directory, import_prefix))
            directory = directory[:-len(import_prefix)]

        include_paths.add(directory)

    wit_file = ctx.actions.declare_file(paths.join(ctx.label.name, "server.wit"))
    metadata_file = ctx.actions.declare_file("metadata.binpb", sibling = wit_file)

    parameters = []
    if ctx.attr.ignore_unsupported_features:
        parameters.append("--vimana_opt=ignore-groups,ignore-required")

    ctx.actions.run(
        executable = ctx.executable._protoc_bin,
        inputs = ctx.files.srcs + include_files,
        outputs = [wit_file, metadata_file],
        arguments = [
            "--plugin={}".format(ctx.executable._protoc_gen_vimana_bin.path),
            "--vimana_out={}".format(wit_file.dirname),
        ] + [
            "--proto_path={}".format(path)
            for path in include_paths
        ] + [
            src.path
            for src in ctx.files.srcs
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
        "srcs": attr.label_list(
            doc = "Source Protobuf definition files.",
            allow_files = [".proto"],
        ),
        "include": attr.label_keyed_string_dict(
            doc = "Protobuf definition files available for import by the source files." +
                  " Keys are proto files or file groups containing proto files." +
                  " Values are the import path prefix" +
                  " under which `protoc` will look for each group of files." +
                  " The import path prefix must appear at the end" +
                  " of the files' common path prefix.",
            allow_files = [".proto"],
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
