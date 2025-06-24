//! IP address management.

use std::fmt::{Display, Result as FmtResult};
use std::io::{pipe, PipeReader, Write};
use std::mem::drop;
use std::net::IpAddr;
use std::simd::u8x16;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{from_slice, json, to_vec};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use logging::log_info;
use names::{hexify, PodName};

/// CNI plugin API version.
/// Seems to be the latest version supported by the `host-local` plugin
/// at time of writing.
const CNI_VERSION: &str = "1.0.0";

// TODO: Verify assumption:
//   Must be the same network name used by the downstream OCI runtime,
//   so IP address allocations do not collide.
/// Default network implementation for Minikube: https://kindnet.es/.
const CNI_NETWORK_NAME: &str = "kindnet";

/// Client to allocate available IP addresses.
#[derive(Clone)]
pub(crate) struct Ipam(Arc<IpamInner>);

/// See [`Ipam`].
struct IpamInner {
    /// Path to a CNI plugin binary to handle IPAM.
    path: String,

    /// Serialized JSON object representing the CNI plugin's configuration:
    /// https://www.cni.dev/docs/spec/#section-1-network-configuration-format.
    config: Vec<u8>,
}

/// An allocated and activated IP address.
///
/// Allocating the address excludes other pods from allocating the same address.
/// Activating it makes it available on the network interface.
#[derive(Clone)]
pub(crate) struct IpAddress {
    /// IPAM client used to allocate this IP address.
    ipam: Ipam,

    /// The allocated IP address.
    pub(crate) address: IpAddr,

    /// Length of the subnet prefix on the local machine.
    prefix_length: u8,

    /// Pod name associated with the IP address.
    pod_name: PodName,

    /// Network interface name, e.g. `eth0`.
    interface: String,
}

impl Ipam {
    /// Create a new IPAM provider using the `host-local` CNI plugin:
    /// https://www.cni.dev/plugins/current/ipam/host-local/.
    pub(crate) fn host_local(path: String, pod_cidr: &str) -> Self {
        let config = to_vec(&json!({
            "cniVersion": CNI_VERSION,
            "name": CNI_NETWORK_NAME,
            "ipam": {
                "type": "host-local",
                // TODO: Verify veracity of this claim:
                // Must be the same data directory used by the downstream OCI runtime,
                // so IP address allocations do not overlap.
                "dataDir": "/run/cni-ipam-state",
                "ranges": [
                    [{"subnet": pod_cidr}],
                ],
            },
        }))
        .unwrap();
        Self(Arc::new(IpamInner { path, config }))
    }

    /// Allocate and return a fresh IP address.
    pub(crate) async fn address(&self, pod_name: &PodName, interface: &str) -> Result<IpAddress> {
        let output = self
            .run_plugin_command("ADD", pod_name)
            .await
            .context("Failed to run IPAM ADD")?;

        let result: IpamAddResult =
            from_slice(&output).context("Error decoding IPAM 'ADD' response")?;
        if result.ips.len() != 1 {
            // We could relax this constraint to allow multiple IP addresses per pod
            // (say, an IPv4 address and an IPv6 address).
            return Err(anyhow!(
                "Multiple IP addresses not supported: {:?}",
                result.ips.len()
            ));
        }
        let cidr = &result.ips.get(0).unwrap().address;

        // The IPAM plugin returns address with a subnet mask for the local machine
        // (e.g. `10.0.0.1/8` intead of just `10.0.0.1`).
        let mut cidr_parts = cidr.split('/');
        let address = cidr_parts
            .next()
            .ok_or_else(|| anyhow!("Invalid address mask: {:?}", cidr))?;
        let prefix_length = cidr_parts
            .next()
            .ok_or_else(|| anyhow!("Invalid address mask: {:?}", cidr))?;
        debug_assert!(cidr_parts.next().is_none());

        // Parse the IP address and prefix length.
        let address: IpAddr = address
            .parse()
            .with_context(|| format!("Invalid address: {:?}", address))?;
        let prefix_length: u8 = prefix_length
            .parse()
            .with_context(|| format!("Invalid subnet prefix length: {:?}", cidr))?;

        // If activating the address on the interface fails,
        // de-allocate it so it can be re-used.
        if let Err(error) = ip_addr("add", &address, prefix_length, interface).await {
            let _ = self.run_plugin_command("DEL", pod_name).await;
            return Err(error);
        }

        Ok(IpAddress {
            ipam: self.clone(),
            address,
            prefix_length,
            pod_name: pod_name.clone(),
            interface: String::from(interface),
        })
    }

    /// Boilerplate to run an IPAM CNI plugin command.
    /// Sets the appropriate parameters and pipes the config to standard input.
    /// On success, return the resulting standard output.
    async fn run_plugin_command(&self, command: &str, pod_name: &PodName) -> Result<Vec<u8>> {
        let output = Command::new(&self.0.path)
            // https://www.cni.dev/docs/spec/#parameters
            // Set parameters, starting with a clean environment (no inheritence).
            .env_clear()
            .env("CNI_COMMAND", command)
            // TODO: The container ID associated with the IP address has to be unique,
            //       but it's unclear whether it might collide with a container ID
            //       generated by the downstream runtime. Verify that a collision is impossible.
            .env("CNI_CONTAINERID", ipam_container_id(&pod_name))
            // CNI plugins require `CNI_NETNS`, `CNI_IFNAME`, or `CNI_PATH` to be non-empty,
            // but these are ignored by the IPAM plugin.
            .env("CNI_NETNS", "/dev/null")
            .env("CNI_IFNAME", "unused")
            .env("CNI_PATH", "/..")
            .stdin(self.config_pipe()?)
            .output()
            .await
            .context("Error executing command")?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(anyhow!(
                "IPAM error: {}",
                // `host-local` prints errors to standard output rather than standard error
                // (as of v1.6.2).
                String::from_utf8_lossy(&output.stdout),
            ))
        }
    }

    /// Create a new unnamed pipe to feed config data to a command's standard input.
    fn config_pipe(&self) -> Result<PipeReader> {
        let (reader, mut writer) = pipe().context("Error creating stdin pipe")?;
        writer
            .write_all(&self.0.config)
            .context("Error writing to stdin pipe")?;
        drop(writer); // Flush the pipe.
        Ok(reader)
    }
}

impl IpAddress {
    /// Deactivate the IP address on its network interface.
    /// It will no longer be able to receive traffic,
    /// but the address will not be available for re-use.
    pub(crate) async fn deactivate(&self) -> Result<()> {
        ip_addr("del", &self.address, self.prefix_length, &self.interface).await?;
        log_info!(
            pod: &self.pod_name,
            "Successful IPAM deactivation: {}",
            self.address,
        );
        Ok(())
    }

    /// De-allocate the address for re-use by other pods.
    /// It must be [deactivated](Self::deactivate) before being de-allocated.
    pub(crate) async fn deallocate(&self) -> Result<()> {
        self.ipam
            .run_plugin_command("DEL", &self.pod_name)
            .await
            .context("Failed to run IPAM DEL")?;
        log_info!(
            pod: &self.pod_name,
            "Successful IPAM deallocation: {}",
            self.address,
        );
        Ok(())
    }
}

impl Display for IpAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> FmtResult {
        self.address.fmt(f)
    }
}

/// Boilerplate to run a command of the form:
///     ip addr <command> <address>/<prefix_length> dev <interface>
#[inline]
async fn ip_addr(
    command: &str,
    address: &IpAddr,
    prefix_length: u8,
    interface: &str,
) -> Result<()> {
    let masked_address = format!("{}/{}", address, prefix_length);
    let output = Command::new("ip")
        .args(["addr", command, &masked_address, "dev", interface])
        .output()
        .await
        .context("Failed to run `ip addr`")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "Error status from `ip addr {} {} dev {}`: {}",
            command,
            masked_address,
            interface,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// The `host-local` IPAM plugin cannot handle characters like `:` and `@` found in pod names.
/// Compute a legal container ID by hashing the pod name and encoding it in hexadecimal.
fn ipam_container_id(pod: &PodName) -> String {
    // Compute the SHA-256 hash of the pod name.
    let mut hasher = Sha256::new();
    hasher.update(pod.to_string());
    let hash = hasher.finalize();

    // SHA-256 hashes are always 32 bytes.
    debug_assert!(hash.len() == 32);
    let mut chunks = hash.chunks_exact(16);
    let mut output = Vec::with_capacity(64);
    output.extend_from_slice(hexify(u8x16::from_slice(chunks.next().unwrap())).as_array());
    output.extend_from_slice(hexify(u8x16::from_slice(chunks.next().unwrap())).as_array());
    unsafe { String::from_utf8_unchecked(output) }
}

/// Used to parse the JSON result of the IPAM plugin for the `ADD` command.
#[allow(non_snake_case)]
#[allow(dead_code)]
#[derive(Deserialize)]
struct IpamAddResult {
    cniVersion: String,
    ips: Vec<IpamAddResultIp>,
}

/// Used to parse the JSON result of the IPAM plugin for the `ADD` command.
/// See [`IpamAddResult`].
#[derive(Deserialize)]
struct IpamAddResultIp {
    address: String,
    gateway: String,
}
