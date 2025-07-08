load("@rules_k8s//:resource.bzl", "K8sResources", "SetupActions")

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

    setup_executables = []
    setup_runfiles = []
    for action in ctx.attr.setup:
        action = action[DefaultInfo]
        setup_executables.append(action.files_to_run.executable)
        setup_runfiles.append(action.default_runfiles)

    return [
        DefaultInfo(files = depset([resources, ca_key, ca_cert])),
        K8sResources(files = depset([resources])),
        SetupActions(
            executables = depset(setup_executables),
            runfiles = ctx.runfiles().merge_all(setup_runfiles),
        ),
    ]

vimana_bootstrap = rule(
    implementation = _vimana_bootstrap_impl,
    doc = "K8s resources necessary to bootstrap a Vimana domain." +
          "This should not be used directly. Invoke it via the macro.",
    provides = [K8sResources, SetupActions],
    attrs = {
        "domains": attr.string(
            doc = "Map from domain IDs to domain objects. " +
                  "The whole thing is JSON-encoded.",
        ),
        "setup": attr.label_list(
            doc = "Executable setup actions to run" +
                  " prior to creating the initial resources during bootstrapping.",
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
