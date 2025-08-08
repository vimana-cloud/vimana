load("@bazel_skylib//lib:paths.bzl", "paths")
load("@bazel_skylib//lib:shell.bzl", "shell")
load("@rules_pkg//pkg:mappings.bzl", "pkg_filegroup", "pkg_files")
load("@rules_pkg//pkg:providers.bzl", "PackageFilegroupInfo")

def pkg_directory(name, subdirectories = None, exclude = None, visibility = None):
    """
    Define a `pkg_filegroup` rule
    that includes all direct child files of the package (except the build file)
    as well as any explicitly named subdirectories,
    each of which is expected to contain a `pkg_filegroup` rule
    with the same name as the subdirectory.
    Directory structure is preserved.
    """
    subdirectories = subdirectories or []
    exclude = exclude or []
    visibility = visibility or []

    package = native.package_name()
    if package == "":
        parent_package = "/"
    else:
        parent = paths.dirname(package)
        parent_package = "//{}".format(parent)
        visibility.append("//{}:__pkg__".format(parent))

    files_name = "{}._files".format(name)
    pkg_files(
        name = files_name,
        srcs = native.glob(
            ["*"],
            allow_empty = True,
            exclude = ["BUILD.bazel"] + exclude,
        ),
    )
    srcs = [":" + files_name]

    for subdirectory in subdirectories:
        subdirectory_name = "{}.{}".format(name, subdirectory)
        pkg_filegroup(
            name = subdirectory_name,
            srcs = ["{}/{}/{}".format(parent_package, name, subdirectory)],
            prefix = "{}/".format(subdirectory),
        )
        srcs.append(":" + subdirectory_name)

    pkg_filegroup(
        name = name,
        srcs = srcs,
        visibility = visibility,
    )

def _helm_template_test_impl(ctx):
    # Consolidate all the files from the `pkg_filegroup` object into a list of runfiles
    # and a single map from filegroup destination paths to runfile paths.
    chart_files = {}
    runfiles = []
    for pkg_files, _ in ctx.attr.chart[PackageFilegroupInfo].pkg_files:
        for path, file in pkg_files.dest_src_map.items():
            chart_files[path] = file.short_path
            runfiles.append(file)

    runner = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.write(
        output = runner,
        content = """
#!/usr/bin/env bash
exec {} --helm={} --chart-files={} --resources={} --target={} --expected={}
""".format(
            shell.quote(ctx.executable._runner.short_path),
            shell.quote(ctx.executable._helm_bin.short_path),
            shell.quote(json.encode(chart_files)),
            shell.quote(ctx.file.resources.short_path),
            shell.quote(ctx.attr.target),
            shell.quote(ctx.file.expected.short_path),
        ),
        is_executable = True,
    )

    runfiles.extend([ctx.executable._helm_bin, ctx.file.resources, ctx.file.expected])
    runfiles = \
        ctx.runfiles(files = runfiles) \
            .merge(ctx.attr._runner[DefaultInfo].default_runfiles)
    return [DefaultInfo(executable = runner, runfiles = runfiles)]

helm_template_test = rule(
    implementation = _helm_template_test_impl,
    doc = "TODO",
    test = True,
    attrs = {
        "chart": attr.label(
            doc = "Full directory of the Helm chart.",
            providers = [PackageFilegroupInfo],
        ),
        "resources": attr.label(
            doc = "YAML file containing custom resources that are relevant to the templates.",
            allow_single_file = [".json", ".yaml"],
        ),
        "target": attr.string(
            doc = "Name of the resource in the resources file" +
                  " whose spec is used as input values for the chart.",
        ),
        "expected": attr.label(
            doc = "Expected output resource YAML file.",
            allow_single_file = [".json", ".yaml"],
        ),
        "_runner": attr.label(
            default = ":helm-template-test-runner",
            executable = True,
            cfg = "exec",
        ),
        "_helm_bin": attr.label(
            executable = True,
            default = "@rules_k8s//:helm",
            allow_single_file = True,
            cfg = "exec",
        ),
    },
)
