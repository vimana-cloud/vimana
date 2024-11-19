def _cluster_test_impl(ctx):
    # Parameterize the runner by expanding the template.
    runner = ctx.actions.declare_file(ctx.label.name + ".runner.sh")
    ctx.actions.expand_template(
        template = ctx.file._runner_template,
        output = runner,
        substitutions = {
            "{{KUBECTL}}": ctx.file._kubectl_bin.short_path,
            "{{OBJECTS}}": json.encode([object.short_path for object in ctx.files.objects]),
            "{{PORT-FORWARD}}": json.encode(ctx.attr.port_forward),
            "{{HOSTS}}": json.encode(ctx.attr.hosts),
            "{{TEST}}": ctx.executable.test.short_path,
        },
        is_executable = True,
    )
    runfiles = \
        ctx.runfiles(files = ctx.files._kubectl_bin + ctx.files.objects) \
            .merge(ctx.attr.test[DefaultInfo].default_runfiles)
    return [
        DefaultInfo(executable = runner, runfiles = runfiles),
        # Inherit `KUBECONFIG` and `HOME` from the host environment
        # so kubectl can find a client configuration.
        RunEnvironmentInfo(inherited_environment = ["KUBECONFIG", "HOME"]),
    ]

cluster_test = rule(
    implementation = _cluster_test_impl,
    doc = "Run an integration test within an existing Kubernetes cluster.",
    test = True,
    attrs = {
        "test": attr.label(
            doc = "Test executable to run within the Minikube cluster.",
            executable = True,
            allow_files = True,
            cfg = "exec",
        ),
        "objects": attr.label_list(
            doc = "Initial Kubernetes API objects defined in YAML files." +
                  " Each object is created before the test is started.",
            allow_files = [".yaml"],
            default = [
                "//gateway:deploy.yaml",
                "//api/v1:deploy.yaml",
                ":tls-api-vimana-host",
            ],
        ),
        "port_forward": attr.string_list_dict(
            doc = "Port forwarding to cluster resources." +
                  " Keys are resource names (e.g. 'svc/vimana-gateway-istio')" +
                  " and values are lists of colon-separated port pairs (e.g. '61803:443').",
            default = {
                "svc/vimana-gateway-istio": ["61803:443"],
            },
        ),
        "hosts": attr.string_dict(
            doc = "Map from hosts (domain names) to IP addresses." +
                  " The contents of /etc/hosts will be overridden with this configuration" +
                  " for the duration of the test." +
                  " Can be used with `port_forward` to enable TLS-encrypted access to services.",
            default = {
                "api.vimana.host": "127.0.0.1",
            },
        ),
        "_runner_template": attr.label(
            default = ":runner.sh",
            allow_single_file = True,
        ),
        "_kubectl_bin": attr.label(
            executable = True,
            default = ":kubectl",
            allow_single_file = True,
            cfg = "exec",
        ),
    },
)

# Convenience macro for [`cluster_test`] when the test binary is Python.
def py_cluster_test(name, srcs):
    #native.pass
    pass

def k8s_secret_tls(name, key, cert):
    """ Convert TLS private key / certificate PEM files into a K8s secret object. """
    key = native.package_relative_label(key)
    cert = native.package_relative_label(cert)
    native.genrule(
        name = name,
        srcs = [key, cert],
        outs = [name + ".yaml"],
        cmd =
            "./$(location :kubectl) create secret tls {} --dry-run=client --key=$(location {}) --cert=$(location {}) --output=yaml > \"$@\""
                .format(name, key, cert),
        tools = [Label(":kubectl"), key, cert],
    )
