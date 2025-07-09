//! Entrypoint to the work node controller.
//!
//! A single instance of this binary runs in each work node
//! and governs the node by serving gRPC services on two ports:
//!
//! - UDP 443 (HTTPS/3)
//!   fields requests from Ingress to all hosted services.
//! - Unix `/run/vimana/workd.sock`
//!   handles orchestration requests from Kubelet.
#![feature(portable_simd)]

mod containers;
mod cri;
mod host;
mod ipam;
mod pods;
mod state;

use std::collections::HashSet;
use std::error::Error as StdError;
use std::fs::{create_dir_all, remove_file, File};
use std::io::BufReader;
use std::path::Path;
use std::result::Result as StdResult;

use anyhow::Context;
use clap::Parser;
use futures::FutureExt;
use hyper_util::rt::TokioIo;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::LoggerProviderBuilder;
use opentelemetry_stdout::LogExporter as StdoutLogExporter;
use serde::Deserialize;
use serde_json::from_reader;
use tokio::net::{UnixListener, UnixStream};
use tokio::select;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Endpoint, Server};
use tower::service_fn;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::Registry;
use wasmtime::{Config as WasmConfig, Engine as WasmEngine};

use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::image_service_server::ImageServiceServer;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::runtime_service_server::RuntimeServiceServer;
use containers::ContainerStore;
use cri::image::ProxyingImageService;
use cri::runtime::{ProxyingRuntimeService, CONTAINER_RUNTIME_NAME, CONTAINER_RUNTIME_VERSION};
use ipam::Ipam;
use state::WorkRuntime;

/// Default value for [`WorkdConfig::incoming`].
const DEFAULT_INCOMING: &str = "/run/vimana/workd.sock";
/// Default value for [`WorkdConfig::downstream`].
const DEFAULT_DOWNSTREAM: &str = "/run/containerd/containerd.sock";
/// Default value for [`WorkdConfig::image_store`].
const DEFAULT_IMAGE_STORE: &str = "/var/lib/vimana/images";
/// Default value for [`WorkdConfig::ipam_plugin`].
const DEFAULT_IPAM_PLUGIN: &str = "/opt/cni/bin/host-local";
/// Default value for [`WorkdConfig::network_interface`].
const DEFAULT_NETWORK_INTERFACE: &str = "eth0";
/// Default value for [`WorkdConfig::pod_ips`].
const DEFAULT_POD_IPS: &str = "10.1.0.0/16";

/// Vimana work node runtime.
///
/// Every option is configurable as a command-line argument or in the configuration file located at `config`.
/// Command-line options take precedence.
#[derive(Parser, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[command(name = CONTAINER_RUNTIME_NAME, version = CONTAINER_RUNTIME_VERSION, verbatim_doc_comment)]
struct WorkdConfig {
    /// Path to the config file
    #[arg(long, value_name = "PATH")]
    config: Option<String>,

    /// Path to the Unix-domain socket
    /// on which to listen for CRI requests from Kubelet
    #[arg(long, value_name = "PATH")]
    incoming: Option<String>,

    /// Path to the Unix-domain socket
    /// to which requests for OCI pods and images are forwarded
    #[arg(long, value_name = "PATH")]
    downstream: Option<String>,

    /// Root filesystem path under which to save pulled images
    #[arg(long, value_name = "PATH")]
    image_store: Option<String>,

    /// Container registries that should be pulled from using HTTP rather than HTTPS
    #[arg(long, value_name = "HOST")]
    insecure_registries: Vec<String>,

    /// Path to a CNI plugin to handle IPAM
    #[arg(long, value_name = "PATH")]
    ipam_plugin: Option<String>,

    /// Name of the network interface to use for data plane traffic
    #[arg(long, value_name = "NAME")]
    network_interface: Option<String>,

    // TODO: This must be coordinated with the downstream runtime
    //   to avoid IP address collisions.
    /// Exclusive subnet for all IP addresses that can be allocated to pods on this node
    #[arg(long, value_name = "CIDR")]
    pod_ips: Option<String>,
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn StdError>> {
    // Read configuration from the command-line first,
    // falling back on the JSON configuration file for unset fields.
    let args = WorkdConfig::parse();
    let config = args.config.map_or(WorkdConfig::default(), |config_path| {
        from_reader(BufReader::new(
            File::open(&config_path)
                .expect(&format!("Error opening config file '{}'", config_path)),
        ))
        .expect(&format!("Error parsing config file '{}'", config_path))
    });

    // Select all options from command-line first, config file second, default value third.
    let incoming = args
        .incoming
        .or(config.incoming)
        .unwrap_or(String::from(DEFAULT_INCOMING));
    let downstream = args
        .downstream
        .or(config.downstream)
        .unwrap_or(String::from(DEFAULT_DOWNSTREAM));
    let image_store = args
        .image_store
        .or(config.image_store)
        .unwrap_or(String::from(DEFAULT_IMAGE_STORE));
    let insecure_registries = args
        .insecure_registries
        .into_iter()
        .chain(config.insecure_registries.into_iter())
        .collect::<HashSet<_>>();
    let ipam_plugin = args
        .ipam_plugin
        .or(config.ipam_plugin)
        .unwrap_or(String::from(DEFAULT_IPAM_PLUGIN));
    let network_interface = args
        .network_interface
        .or(config.network_interface)
        .unwrap_or(String::from(DEFAULT_NETWORK_INTERFACE));
    let pod_ips = args
        .pod_ips
        .or(config.pod_ips)
        .unwrap_or(String::from(DEFAULT_POD_IPS));

    let logger_provider = LoggerProviderBuilder::default()
        .with_simple_exporter(StdoutLogExporter::default())
        .build();
    Registry::default()
        .with(LevelFilter::INFO)
        .with(OpenTelemetryTracingBridge::new(&logger_provider))
        .init();

    // This seems to be the most idiomatic way to create a client with a UDS transport:
    // https://github.com/hyperium/tonic/blob/v0.12.3/examples/src/uds/client.rs.
    // The socket path must be cloneable to enable re-invoking the connector function.
    let oci_socket_path = downstream.clone();
    let oci_channel = Endpoint::from_static("http://unused")
        .connect_with_connector(service_fn(move |_| {
            let oci_socket_path = oci_socket_path.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(&oci_socket_path).await?))
            }
        }))
        .await
        .context(format!(
            "Unable to connect to OCI runtime socket: {:?}",
            downstream
        ))?;
    let oci_image_client = ImageServiceClient::new(oci_channel.clone());
    let oci_runtime_client = RuntimeServiceClient::new(oci_channel);

    let ipam = Ipam::host_local(ipam_plugin, &pod_ips, network_interface).await?;

    // systemd sends SIGTERM to stop services, CTRL+C sends SIGINT.
    // Listen for those to shut down the servers somewhat gracefully.
    let mut sigterm = signal(SignalKind::terminate())
        .unwrap_or_else(|err| panic!("Cannot listen for SIGTERM: {err}"));
    let mut sigint = signal(SignalKind::interrupt())
        .unwrap_or_else(|err| panic!("Cannot listen for SIGINT: {err}"));
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_signal = async move {
        select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }
        // Re-broadcast the shutdown signal to the data plane (best effort).
        let _ = shutdown_tx.send(());
    };

    // A new instance of the default engine for this runtime.
    let wasmtime = WasmEngine::new(
        WasmConfig::new()
            // Allow host functions to be `async` Rust.
            // Means you have to use `Func::call_async` instead of `Func::call`.
            .async_support(true)
            // Epoch interruption for preemptive multithreading.
            // https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html#method.epoch_interruption
            //.epoch_interruption(true)
            // Enable support for various Wasm proposals...
            .wasm_component_model(true)
            .wasm_gc(true)
            .wasm_tail_call(true)
            .wasm_function_references(true),
    )?;

    let containers = ContainerStore::new(&image_store, insecure_registries, &wasmtime)?;
    let runtime = WorkRuntime::new(wasmtime, containers.clone(), ipam, shutdown_rx.shared());

    // Bind to our CRI API socket.
    // This is last fallible thing before starting the CRI API server
    // because any failures that occur after this should cause the socket to be unlinked
    // so the service can be restarted successfully.
    create_dir_all(Path::new(&incoming).parent().unwrap())?;
    let cri_listener =
        UnixListener::bind(&incoming).expect(&format!("Cannot bind Unix socket '{}'", &incoming));

    let result = Server::builder()
        .add_service(RuntimeServiceServer::new(ProxyingRuntimeService::new(
            runtime,
            oci_runtime_client,
        )))
        .add_service(ImageServiceServer::new(ProxyingImageService::new(
            containers,
            oci_image_client,
        )))
        .serve_with_incoming_shutdown(UnixListenerStream::new(cri_listener), shutdown_signal)
        .await;

    // Remove the UDS path after shutdown so we can rebind on restart.
    // Do this before propagating potential CRI API server errors.
    let unlink_socket_result = remove_file(&incoming);

    result?;
    Ok(unlink_socket_result?)
}
