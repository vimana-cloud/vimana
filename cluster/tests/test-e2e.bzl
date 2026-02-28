"""Convenience macro and helper functions to handle boilerplate in defining E2E tests."""

load("@rules_k8s//:test.bzl", "k8s_cluster_test")
load("//cluster/bootstrap:bootstrap.bzl", "vimana_image_push")

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

def e2e_test(
        name,
        executable,
        vimana,
        superdomain = "",
        domains = {},
        setup = None,
        resources = [],
        tags = [],
        **kwargs):
    """
    Convenience macro to set up and define an end-to-end test.

    Args:
        name: Name for the E2E test target.
        executable: Exectuable target that exercises and tests a Vimana cluster.
        vimana: The name of the Vimana resource for this test.
        superdomain: The Vimana's canonical superdomain,
                     used to derive canonical domain names for TLS certificates.
                     Must match the `canonicalSuperdomain` in the Vimana spec.
        domains: Mapping from domain IDs (e.g. `0123456789abcdef0123456789abcdef`)
                 to relevant resource configurations for each Vimana domain defined in `resources`.
                 Each value must be a result returned by the `domain` function.
        setup: List of executable targets to run at the beginning of the test
               to set up the environment.
        resources: List of K8s resource files (YAML or JSON)
                   to apply in the cluster before running the test.
        superdomain: The Vimana's canonical superdomain,
                     used to derive canonical domain names for TLS certificates.
                     Must match the `canonicalSuperdomain` in the Vimana spec.
    """
    setup = setup or [
        Label("//cluster/tests:issuer-setup"),
        Label("//cluster/tests/setup:deploy-etcd-for-external-dns"),
        Label("//cluster/tests/setup:install-external-dns"),
        Label("//cluster/tests/setup:patch-corefile"),
    ]
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
                setup.append(":{}".format(push_name))

        if superdomain:
            domain_names.add("{}.{}".format(domain_id, superdomain))
        for alias in domain.aliases:
            domain_names.add(alias)

    # The Vimana operator always gives the Gateway the same name as the Vimana
    # plus the `-gateway` suffix.
    gateway_name = "{}-gateway".format(vimana)
    k8s_cluster_test(
        name = name,
        objects = resources,
        gateway_domains = {
            gateway_name: domain_names,
        },
        gateway_service_selectors = {
            gateway_name: "gateway.envoyproxy.io/owning-gateway-name={}".format(gateway_name),
        },
        setup = setup,
        # The `external` tag causes the test to never cache its results.
        # There is no practical way to manage the test cache with an external cluster.
        tags = tags + ["external"],
        test = executable,
        **kwargs
    )
