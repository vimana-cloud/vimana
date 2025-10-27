# Rules to build a binary for a Docker container,
# which typically requires a specific platform regardless of the host.
# Crazy that we need 50+ lines of Starlark just to set the platform.
#
# Inspired by
# https://github.com/bazelbuild/platforms/blob/0.0.11/experimental/platform_data/defs.bzl.
# TODO: Upstream?

def _platform_binary_transition_impl(settings, attr):
    return {"//command_line_option:platforms": str(attr.platform)}

_platform_binary_transition = transition(
    implementation = _platform_binary_transition_impl,
    inputs = [],
    outputs = [
        "//command_line_option:platforms",
    ],
)

def _platform_binary_impl(ctx):
    info = ctx.attr.target[0][DefaultInfo]
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

platform_binary = rule(
    implementation = _platform_binary_impl,
    attrs = {
        "target": attr.label(
            doc = "The target to transition.",
            allow_files = False,
            executable = True,
            mandatory = True,
            cfg = _platform_binary_transition,
        ),
        "platform": attr.label(
            doc = "The platform to transition to.",
            mandatory = True,
        ),
    },
    executable = True,
)
