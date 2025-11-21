"""
Provide a transition rule to override operator configuration at build time.

The transition is applied to the `:deploy` executable rule in this package by default.
"""

def _operator_deploy_configure_transition_impl(settings, attr):
    return {"//operator:image-name": attr.image_name}

_operator_deploy_configure_transition = transition(
    implementation = _operator_deploy_configure_transition_impl,
    inputs = [],
    outputs = ["//operator:image-name"],
)

def _operator_deploy_configure_impl(ctx):
    info = ctx.attr._target[0][DefaultInfo]
    output = ctx.actions.declare_file(ctx.attr.name)

    ctx.actions.symlink(
        output = output,
        target_file = info.files_to_run.executable,
        is_executable = True,
    )

    return [
        DefaultInfo(
            files = depset([output]),
            runfiles = info.default_runfiles,
            executable = output,
        ),
    ]

operator_deploy_configure = rule(
    implementation = _operator_deploy_configure_impl,
    attrs = {
        "image_name": attr.string(
            doc = "Name for the image (including registry).",
            mandatory = True,
        ),
        "_target": attr.label(
            doc = "The target to transition.",
            default = ":deploy",
            allow_files = False,
            executable = True,
            cfg = _operator_deploy_configure_transition,
        ),
    },
    executable = True,
)
