load("@bazel_skylib//lib:shell.bzl", "shell")
load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("//compiler:defs.bzl", "MetadataInfo")

VIMANA_IMAGE_PUSH_SCRIPT_TEMPLATE = (
    "#!/usr/bin/env bash\n" +
    "{} --repository={} --version={} --component={} --metadata={}{}\n"
)

def _vimana_image_push_impl(ctx):
    metadata_file = ctx.attr.metadata[MetadataInfo].file
    repository = ctx.expand_make_variables("repository", ctx.attr.repository, {})
    insecure_registries = []
    if ctx.attr.insecure_registries:
        insecure_registries = ctx.attr.insecure_registries[BuildSettingInfo].value

    runner = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.write(
        output = runner,
        content = VIMANA_IMAGE_PUSH_SCRIPT_TEMPLATE.format(
            shell.quote(ctx.executable._push_image_bin.short_path),
            shell.quote(repository),
            shell.quote(ctx.attr.version),
            shell.quote(ctx.file.component.short_path),
            shell.quote(metadata_file.short_path),
            "".join([
                " --insecure-registry={}".format(shell.quote(domain))
                for domain in insecure_registries
            ]),
        ),
        is_executable = True,
    )
    runfiles = ctx.runfiles(
        files = [ctx.file.component, metadata_file],
    ).merge(
        ctx.attr._push_image_bin[DefaultInfo].default_runfiles,
    )
    return [
        DefaultInfo(executable = runner, runfiles = runfiles),
        # Inherit the environment variables used to load Docker credential helpers.
        # https://docs.docker.com/reference/cli/docker/#environment-variables
        # https://github.com/docker/cli/pull/6008
        # TODO: Add support for `DOCKER_AUTH_CONFIG` too?
        RunEnvironmentInfo(
            inherited_environment = ["DOCKER_CONFIG", "HOME"],
        ),
    ]

vimana_image_push = rule(
    executable = True,
    implementation = _vimana_image_push_impl,
    doc =
        "Push a Vimana container," +
        " consisting of a component module and matching metadata," +
        " to the given OCI container repository.",
    attrs = {
        "component": attr.label(
            doc = "Compiled component module.",
            allow_single_file = [".wasm"],
        ),
        "metadata": attr.label(
            doc = "Serialized metadata. This must be the output of a `vimana_protoc` rule.",
            providers = [MetadataInfo],
        ),
        "repository": attr.string(
            doc = "Repository to push the image to, e.g. `localhost:5000/image-name`." +
                  " Subject to \"Make variable\" substitution.",
        ),
        "version": attr.string(
            doc = "Component version, e.g. `1.0.0-release`. Must be a valid SemVer." +
                  " Used as the tag for the push.",
        ),
        "insecure_registries": attr.label(
            doc = "Repeatable string-valued build setting containing the domains of registries" +
                  " for which to use cleartext HTTP instead of HTTPS.",
            providers = [BuildSettingInfo],
        ),
        "_push_image_bin": attr.label(
            default = ":push-image",
            executable = True,
            cfg = "exec",
        ),
    },
)
