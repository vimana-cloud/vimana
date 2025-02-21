# Rules to build a binary for a Docker container,
# which typically requires a specific platform regardless of the host.
# Crazy that we need 50+ lines of Starlark just to set the platform.
#
# Heavily influenced by
# https://github.com/bazelbuild/platforms/blob/0.0.11/experimental/platform_data/defs.bzl.
# TODO: Upstream?

def _platform_executable_transition_impl(settings, attr):
    return {
        "//command_line_option:platforms": str(attr.platform),
    }

_platform_executable_transition = transition(
    implementation = _platform_executable_transition_impl,
    inputs = [],
    outputs = [
        "//command_line_option:platforms",
    ],
)

def _platform_executable_impl(ctx):
    target = ctx.attr.target[0]

    default_info = target[DefaultInfo]
    files = default_info.files
    original_executable = default_info.files_to_run.executable
    runfiles = default_info.default_runfiles

    new_executable = ctx.actions.declare_file(ctx.attr.name)

    ctx.actions.symlink(
        output = new_executable,
        target_file = original_executable,
        is_executable = True,
    )

    return [
        DefaultInfo(
            files = depset([new_executable]),
            runfiles = runfiles.merge(ctx.runfiles([new_executable])),
            executable = new_executable,
        ),
    ]

platform_executable = rule(
    implementation = _platform_executable_impl,
    attrs = {
        "target": attr.label(
            allow_files = False,
            executable = True,
            mandatory = True,
            cfg = _platform_executable_transition,
        ),
        "platform": attr.label(
            mandatory = True,
        ),
    },
    executable = True,
)
