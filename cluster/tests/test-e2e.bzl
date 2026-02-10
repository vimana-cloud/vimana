"""Convenience macro and helper functions to handle boilerplate in defining E2E tests."""

load("@rules_k8s//:test.bzl", "k8s_cluster_test")
load("//cluster/bootstrap:bootstrap.bzl", "self_signed_tls", "vimana_image_push")

def domain(servers = {}, aliases = []):
    """
    Helper method to configure seeded servers for an `e2e_test`.

    Args:
        servers: Mapping from server IDs (e.g. `my-server`)
                 to relevant server configurations.
                 Each value must be a result returned by the `server` function.
        aliases: List of DNS names serving as aliases for this domain.
    """
    return struct(
        servers = servers,
        aliases = aliases,
    )

def server(versions = {}):
    """
    Helper method to configure seeded servers for an `e2e_test`.

    Args:
        versions: Mapping from component version strings (e.g. `1.2.3`)
                  to relevant component configurations.
                  Each value must be a result returned by the `component` function.
    """
    return struct(
        versions = versions,
    )

def component(implementation, metadata):
    """
    Helper method to configure seeded domains for an `e2e_test`.

    Args:
        implementation: Label of the Wasm component implementation.
        metadata: Label of the metadata produced by the compiler plugin for this component.
    """
    return struct(
        implementation = implementation,
        metadata = metadata,
    )

def e2e_test(name, executable, gateway, domains = {}, resources = [], **kwargs):
    """
    Convenience macro to set up and define an end-to-end test.

    Args:
        name: Name for the E2E test target.
        executable: Exectuable target that exercises and tests a Vimana cluster.
        gateway: Name of the single gateway defined in the seed resources.
        domains: Mapping from domain IDs (e.g. `0123456789abcdef0123456789abcdef`)
                 to relevant resource configurations for each Vimana domain defined in `resources`.
                 Each value must be a result returned by the `domain` function.
        resources: List of K8s resource files (YAML or JSON)
                   to apply in the cluster before running the test.
    """
    push_names = []
    domain_names = set()
    for domain_id, domain in domains.items():
        for server_id, server in domain.servers.items():
            for version, component in server.versions.items():
                push_name = "{}.push.{}.{}.{}".format(name, domain_id, server_id, version)
                vimana_image_push(
                    name = push_name,
                    component = component.implementation,
                    metadata = component.metadata,
                    repository = "$(REGISTRY_HOST)/{}/{}".format(domain_id, server_id),
                    toolchains = [Label(":registry-host")],
                    version = version,
                    insecure_registries = Label(":registries-insecure"),
                )
                push_names.append(push_name)

        domain_names.add("{}.app.vimana.host".format(domain_id))
        for alias in domain.aliases:
            domain_names.add(alias)

    certificates_name = "{}.certificates".format(name)
    self_signed_tls(
        name = certificates_name,
        domains = domain_names,
    )

    k8s_cluster_test(
        name = name,
        objects = resources + [":{}".format(certificates_name)],
        services = {
            gateway: domain_names,
        },
        setup = [":{}".format(push_name) for push_name in push_names],
        tags = ["external"],  # Never cache test results.
        test = executable,
        **kwargs
    )
