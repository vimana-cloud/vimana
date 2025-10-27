"""
A module extension and repository rule
configuring information about the environment Bazel is running in.
"""

def _environment_repository_impl(repository_ctx):
    # Detect if Bazel is running inside a Docker container by checking for `/.dockerenv`
    # and provide a hostname for the host machine regardless of containerization.
    containerized = repository_ctx.path("/.dockerenv").exists
    localhost = "host.docker.internal" if containerized else "localhost"

    repository_ctx.file(
        "BUILD.bazel",
        content = "exports_files([\"environment.bzl\"])",
        executable = False,
    )
    repository_ctx.file(
        "environment.bzl",
        content = "localhost = {}".format(repr(localhost)),
        executable = False,
    )

environment_repository = repository_rule(
    implementation = _environment_repository_impl,
    doc = "Configure settings based on the environment Bazel is running in.",
    # This rule fetches everything from the local system.
    local = True,
)

def _environment_impl(module_ctx):
    environment_repository(name = "environment")

environment = module_extension(
    implementation = _environment_impl,
    doc = "Configure settings based on the environment Bazel is running in.",
)
