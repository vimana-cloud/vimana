def _vimana_bootstrap_impl(ctx):
    config = ctx.actions.declare_file(ctx.label.name + ".config.json")
    resources = ctx.actions.declare_file(ctx.label.name + ".json")
    ca_prefix = ctx.label.name + ".root"
    ca_key = ctx.actions.declare_file(ca_prefix + ".key")
    ca_cert = ctx.actions.declare_file(ca_prefix + ".cert", sibling = ca_key)
    ca_path_prefix = ca_key.path.rpartition(".")[0]

    ctx.actions.write(
        output = config,
        content = ctx.attr.domains,
    )
    ctx.actions.run(
        inputs = [config],
        outputs = [resources, ca_key, ca_cert],
        executable = ctx.executable._bootstrap_bin,
        arguments = [
            config.path,
            resources.path,
            "--registry",
            ctx.attr.cluster_registry,
            "--generate-ca",
            ca_path_prefix,
        ],
        tools = ctx.attr._bootstrap_bin[DefaultInfo].default_runfiles.files,
    )

    return [DefaultInfo(files = depset([resources, ca_key, ca_cert]))]

vimana_bootstrap = rule(
    implementation = _vimana_bootstrap_impl,
    doc = "K8s resources necessary to bootstrap a Vimana domain." +
          "This should not be used directly. Invoke it via the macro.",
    attrs = {
        "domains": attr.string(
            doc = "Map from domain IDs to domain objects. " +
                  "The whole thing is JSON-encoded.",
        ),
        "cluster_registry": attr.string(
            doc = "Registry URL to use from within the cluster.",
        ),
        "_bootstrap_bin": attr.label(
            default = ":bootstrap",
            executable = True,
            cfg = "exec",
        ),
    },
)
