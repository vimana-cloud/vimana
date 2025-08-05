load("@bazel_skylib//lib:shell.bzl", "shell")
load(":private.bzl", "vimana_bootstrap")

def bootstrap(name, domains, registry, cluster_registry = None):
    """
    Statically build, push and deploy Vimana cluster resources.
    Used to bootstrap a cluster with the API and any other pre-existing services.

    Defines an executable `vimana_image_push` rule for each component
    to push its module and metadata to the image registry based at `registry`.
    Also defines a single buildable rule for the K8s resources of the cluster.

    Parameters:
        name (str): Name for the build rule for the K8s resources.
        domains ({str: domain}):
            Map from canonical domain IDs (e.g. `0123456789abcdef0123456789abcdef`)
            to lists of objects returned by the `domain` macro.
        registry (str): Container image registry URL root, e.g. `http://localhost:5000`.
        cluster_registry (str): Registry URL to use from within the cluster;
                                default is to use the same value as `registry`.
    """

    # One executable push action per component.
    image_push_actions = []
    for domain_id, domain in domains.items():
        for service_name, service in domain.services.items():
            for component_version, component in service.components.items():
                # Bazel rule names cannot contain a colon,
                # so use dashes instead of the canonical component name.
                image_push_action_name = \
                    "{}.{}-{}-{}".format(name, domain_id, service_name, component_version)
                vimana_image_push(
                    name = image_push_action_name,
                    component = component.module,
                    metadata = component.metadata,
                    domain_id = domain_id,
                    service = service_name,
                    version = component_version,
                    registry = registry,
                )
                image_push_actions.append(":" + image_push_action_name)

    # One overall build rule for the cluster's K8s resources.
    vimana_bootstrap(
        name = name,
        domains = json.encode(domains),
        setup = image_push_actions,
        cluster_registry = cluster_registry or registry,
    )

def domain(aliases = None, services = None, reflection = False):
    """
    Return a domain object that can be used with the `bootstrap` macro.

    Parameters:
        aliases ([str]): List of domain aliases, e.g. `[example.com, example.net]`.
        services ({str: service}):
            Map from service names (e.g. `package.foo.BarService`)
            to objects returned by the `service` macro.
        reflection (bool): Whether to enable reflection on every service in the domain.
    """
    return struct(
        aliases = aliases or [],
        services = services or {},
        reflection = reflection,
    )

def service(components):
    """
    Return a service object that can be used with the `domain` macro.

    Parameters:
        components ({str: component}):
            Map from SemVer version strings (e.g. `1.0.0`)
            to objects returned by the `component` macro.
    """
    return struct(
        components = components,
    )

def component(module, metadata, weight = 1):
    """
    Return a component object that can be used with the `service` macro.

    Parameters:
        module (str): Label of a compiled component module.
        metadata (str): Label of a binary protobuf metadata file.
        weight (int):
            Relative weight of the component for routing traffic
            (normalized against all weights within a service).
    """
    return struct(
        module = module,
        metadata = metadata,
        weight = weight,
    )

def env_from_field_ref(name, field_path):
    """ Return an EnvVar object loading the value from a field reference. """

    # https://kubernetes.io/docs/reference/generated/kubernetes-api/v1.32/#envvar-v1-core
    return {
        "name": name,
        "valueFrom": {
            "fieldRef": {
                "fieldPath": field_path,
            },
        },
    }

def _vimana_image_push_impl(ctx):
    runner = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.write(
        output = runner,
        content = "#!/usr/bin/env bash\n{} {} {} {} {} {} {}".format(
            shell.quote(ctx.file._image_push_bin.short_path),
            shell.quote(ctx.attr.registry),
            shell.quote(ctx.attr.domain_id),
            shell.quote(ctx.attr.service),
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
        "service": attr.string(
            doc = "Service name, e.g. `some.package.FooService`.",
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
