"""Build rule for expanding Go templates with Sprig functions."""

def _expand_sprig_impl(ctx):
    dot = ctx.expand_location(ctx.attr.dot, ctx.attr.deps)
    dot = ctx.expand_make_variables("dot", dot, {})

    output = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.run_shell(
        command = "{} $1 < $2 > $3".format(ctx.executable._expand_sprig_bin.path),
        tools = [ctx.executable._expand_sprig_bin],
        inputs = depset(
            [ctx.file.template],
            transitive = [dep.files for dep in ctx.attr.deps],
        ),
        outputs = [output],
        arguments = [dot, ctx.file.template.path, output.path],
    )

    return [DefaultInfo(files = depset([output]))]

_expand_sprig = rule(
    implementation = _expand_sprig_impl,
    doc = "Execute a Go template with Sprig functions.",
    attrs = {
        "template": attr.label(
            allow_single_file = True,
            mandatory = True,
            doc = "Template file to expand.",
        ),
        "dot": attr.string(
            default = "{}",
            doc = "JSON-encoded data to use during template execution." +
                  " Subject to `$(location)` and \"Make\" variable expansion.",
        ),
        "deps": attr.label_list(
            allow_files = True,
            doc = "Runtime file dependencies.",
        ),
        "_expand_sprig_bin": attr.label(
            default = "//dev/bin:expand-sprig",
            executable = True,
            cfg = "exec",
        ),
    },
)

def expand_sprig(dot, **kwargs):
    kwargs["dot"] = json.encode(dot)
    _expand_sprig(**kwargs)
