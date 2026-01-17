# Networking

Vimana uses a simplified networking model
that bypasses traditional container networking complexity.

Traditional containers need network namespaces for isolation.
Vimana workloads are WASM components
where the runtime controls all network access through WASI capabilities,
obviating the need for kernel-level sandboxing and namespaces.

## How It Works

1. The [`host-local`] CNI plugin allocates an IP from the node's pod CIDR.
2. The IP is added directly to the node's network interface via netlink.
3. The Vimana pod's gRPC server binds to that IP in the host network namespace.

[`host-local`]: https://www.cni.dev/plugins/current/ipam/host-local/

## Routing

Vimana only needs Layer 3 routing between nodes.

| Environment | Solution                           |
|-------------|------------------------------------|
| Kind        | Static routes at [cluster startup] |
| AWS         | [VPC route tables]                 |
| GCP         | [Alias IP ranges]                  |
| Azure       | [User-defined routes]              |

[cluster startup]: /dev/kind/restart.sh
[VPC route tables]: https://docs.aws.amazon.com/vpc/latest/userguide/VPC_Route_Tables.html
[Alias IP ranges]: https://docs.cloud.google.com/vpc/docs/alias-ip
[User-defined routes]: https://learn.microsoft.com/en-us/azure/virtual-network-manager/overview
