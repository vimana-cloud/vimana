load("@bazel_skylib//lib:shell.bzl", "shell")
load("@rules_k8s//:resource.bzl", "K8sResources")

def _vimana_image_push_impl(ctx):
    runner = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.write(
        output = runner,
        content = "#!/usr/bin/env bash\n{} {} {} {} {} {} {}".format(
            shell.quote(ctx.file._image_push_bin.short_path),
            shell.quote(ctx.attr.registry),
            shell.quote(ctx.attr.domain_id),
            shell.quote(ctx.attr.server_id),
            shell.quote(ctx.attr.version),
            shell.quote(ctx.file.component.short_path),
            shell.quote(ctx.file.metadata.short_path),
        ),
        is_executable = True,
    )
    runfiles = ctx.runfiles(
        files = [ctx.file._image_push_bin, ctx.file.component, ctx.file.metadata],
    )
    return [DefaultInfo(executable = runner, runfiles = runfiles)]

vimana_image_push = rule(
    executable = True,
    implementation = _vimana_image_push_impl,
    doc =
        "Push a Vimana container," +
        " consisting of a component module and matching metadata," +
        " to the given OCI container registry.",
    attrs = {
        "component": attr.label(
            doc = "Compiled component module.",
            allow_single_file = [".wasm"],
        ),
        "metadata": attr.label(
            doc = "Serialized metadata.",
            allow_single_file = [".binpb"],
        ),
        "domain_id": attr.string(
            doc = "Domain ID, e.g. `1234567890abcdef1234567890abcdef`.",
        ),
        "server_id": attr.string(
            doc = "Server ID, e.g. `some-server`.",
        ),
        "version": attr.string(
            doc = "Component version, e.g. `1.0.0-release`.",
        ),
        "registry": attr.string(
            doc = "Image registry root, e.g. `http://localhost:5000`.",
        ),
        "_image_push_bin": attr.label(
            default = ":image-push.sh",
            executable = True,
            cfg = "exec",
            allow_single_file = True,
        ),
    },
)

def _self_signed_tls_impl(ctx):
    resources = ctx.actions.declare_file("{}.json".format(ctx.label.name))
    ca_prefix = "{}.root".format(ctx.label.name)
    ca_key = ctx.actions.declare_file("{}.key".format(ca_prefix))
    ca_cert = ctx.actions.declare_file("{}.cert".format(ca_prefix), sibling = ca_key)

    ctx.actions.run(
        executable = ctx.executable._self_signed_tls_bin,
        outputs = [resources, ca_key, ca_cert],
        arguments = [
            ctx.executable._tls_generate_bin.path,
            ctx.executable._openssl_bin.path,
            ca_key.path,
            ca_cert.path,
            resources.path,
        ] + ctx.attr.domains,
        tools = [
            ctx.executable._tls_generate_bin,
            ctx.executable._openssl_bin,
        ],
    )

    runfiles = ctx.runfiles(
        files = [],
    ).merge(ctx.attr._tls_generate_bin[DefaultInfo].default_runfiles)

    return [
        DefaultInfo(files = depset([resources, ca_key, ca_cert]), runfiles = runfiles),
        K8sResources(files = depset([resources])),
    ]

self_signed_tls = rule(
    implementation = _self_signed_tls_impl,
    doc =
        "Generate TLS certificates for a set of domain names," +
        " encoding them as Kubernetes Secret resources," +
        " using a freshly-generated self-signed root CA." +
        " Output three files:" +
        " the resources file and the root CA key pair.",
    attrs = {
        "domains": attr.string_list(
            doc = "List of domain names to generate TLS certificates for.",
        ),
        "_self_signed_tls_bin": attr.label(
            default = ":self-signed-tls",
            executable = True,
            cfg = "exec",
        ),
        "_openssl_bin": attr.label(
            default = "@openssl",
            executable = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "_tls_generate_bin": attr.label(
            default = "@rules_k8s//:tls-generate",
            executable = True,
            cfg = "exec",
        ),
    },
    provides = [K8sResources],
)
