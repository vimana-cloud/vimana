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
use std::sync::Arc;

use clap::Parser;
use futures::FutureExt;
use hyper_util::rt::TokioIo;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::LoggerProvider;
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

/// Path to the JSON config file
/// containing the list of insecure registries.
const CONFIG_PATH: &str = "/etc/workd/config.json";

#[derive(Parser)]
#[command(name = CONTAINER_RUNTIME_NAME, version = CONTAINER_RUNTIME_VERSION)]
struct Args {
    /// Path to the Unix-domain socket
    /// on which the work node runtime listens for CRI requests from the Kubelet.
    /// This should probably be '/run/vimana/workd.sock'.
    #[arg(long, value_name = "PATH")]
    incoming: String,

    /// Path to the Unix-domain socket
    /// to which the requests for OCI pods and images are forwarded.
    #[arg(long, value_name = "PATH")]
    downstream: String,

    /// Path to a CNI plugin binary to handle IPAM,
    /// such as [`host-local`](https://www.cni.dev/plugins/current/ipam/host-local/).
    #[arg(long, value_name = "PATH")]
    ipam_plugin: String,

    /// Name of the network interface to use (e.g. `eth0`).
    #[arg(long, value_name = "NAME")]
    network_interface: String,

    // TODO: This must be coordinated with the downstream runtime
    //   to avoid IP address collisions.
    /// Exclusive subnet for all IP addresses that can be allocated to pods on this node
    /// (e.g. `10.0.1.0/24` or `fc00:0001::/32`).
    #[arg(long, value_name = "CIDR")]
    pod_ips: String,
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn StdError>> {
    let args = Args::parse();

    let logger_provider = LoggerProvider::builder()
        .with_simple_exporter(StdoutLogExporter::default())
        .build();
    Registry::default()
        .with(OpenTelemetryTracingBridge::new(&logger_provider))
        .init();

    // Read the JSON config file.
    let config = if let Ok(config_file) = File::open(CONFIG_PATH) {
        from_reader(BufReader::new(config_file))?
    } else {
        WorkdConfig::default()
    };

    // This seems to be the most idiomatic way to create a client with a UDS transport:
    // https://github.com/hyperium/tonic/blob/v0.12.3/examples/src/uds/client.rs.
    // The socket path must be cloneable to enable re-invoking the connector function.
    let oci_socket_path = Arc::new(args.downstream);
    let oci_channel = Endpoint::from_static("http://unused")
        .connect_with_connector(service_fn(move |_| {
            let oci_socket_path = oci_socket_path.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(
                    UnixStream::connect(oci_socket_path.as_ref()).await?,
                ))
            }
        }))
        .await?;
    let oci_image_client = ImageServiceClient::new(oci_channel.clone());
    let oci_runtime_client = RuntimeServiceClient::new(oci_channel);

    let ipam = Ipam::host_local(args.ipam_plugin, &args.pod_ips);

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

    let containers = ContainerStore::new(config.insecureRegistries, &wasmtime);
    let runtime = WorkRuntime::new(
        wasmtime,
        containers.clone(),
        args.network_interface,
        ipam,
        shutdown_rx.shared(),
    );

    // Bind to our CRI API socket.
    // This is last fallible thing before starting the CRI API server
    // because any failures that occur after this should cause the socket to be unlinked
    // so the service can be restarted successfully.
    create_dir_all(Path::new(&args.incoming).parent().unwrap())?;
    let cri_listener = UnixListener::bind(&args.incoming)
        .expect(&format!("Cannot bind Unix socket '{}'", &args.incoming));

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
    let unlink_socket_result = remove_file(&args.incoming);

    result?;
    Ok(unlink_socket_result?)
}

/// JSON configuration file.
#[allow(non_snake_case)]
#[derive(Deserialize, Default)]
struct WorkdConfig {
    insecureRegistries: HashSet<String>,
}
