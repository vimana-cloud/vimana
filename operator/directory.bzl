load("@bazel_skylib//lib:shell.bzl", "shell")

def _collect_directory_impl(ctx):
    output = ctx.actions.declare_directory(ctx.label.name)

    ctx.actions.run_shell(
        outputs = [output],
        inputs = ctx.files.srcs,
        command = "cp -rL \"$@\" {}".format(shell.quote(output.path)),
        arguments = [src.path for src in ctx.files.srcs],
    )

    return [DefaultInfo(files = depset([output]))]

collect_directory = rule(
    implementation = _collect_directory_impl,
    doc = "Collect files and subdirectories into a directory." +
          " Unlike built-in filegroups or Skylib's directory," +
          " this rule actually copies files" +
          " and can thus be used to combine source files and build outputs.",
    attrs = {
        "srcs": attr.label_list(
            doc = "Files, including subdirectories, to collect.",
            allow_files = True,
        ),
    },
)

def source_files_except_build_and_templates():
    """
    Convenience macro to express a glob of all files in the current directory
    excluding 'BUILD.bazel' and anything with a '.tmpl' extension.
    """
    return native.glob(
        ["*"],
        allow_empty = True,
        exclude = [
            "BUILD.bazel",
            "*.tmpl",
        ],
    )
