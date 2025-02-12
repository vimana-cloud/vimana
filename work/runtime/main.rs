//! Entrypoint to the work node controller.
//!
//! A single instance of this binary runs in each work node
//! and governs the node by serving gRPC services on two ports:
//!
//! - UDP 443 (HTTPS/3)
//!   fields requests from Ingress to all hosted services.
//! - Unix `/run/vimana/workd.sock`
//!   handles orchestration requests from Kubelet.

use std::error::Error as StdError;
use std::fs::{create_dir_all, remove_file};
use std::path::Path;
use std::result::Result as StdResult;
use std::sync::Arc;

use clap::Parser;
use futures::FutureExt;
use hyper_util::rt::TokioIo;
use log::set_boxed_logger;
use opentelemetry_appender_log::OpenTelemetryLogBridge;
use opentelemetry_sdk::logs::{BatchLogProcessor, LoggerProvider};
use opentelemetry_sdk::runtime::Tokio as TokioOtelRuntime;
use opentelemetry_stdout::LogExporter as StdoutLogExporter;
use tokio::net::{UnixListener, UnixStream};
use tokio::select;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Endpoint, Server};
use tower::service_fn;

use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::image_service_server::ImageServiceServer;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::runtime_service_server::RuntimeServiceServer;
use cri::{VimanaCriService, CONTAINER_RUNTIME_NAME, CONTAINER_RUNTIME_VERSION};
use state::WorkRuntime;

/// Cache up to 1 GiB by default (meassured in KiB).
const DEFAULT_CONTAINER_CACHE_MAX_CAPACITY: u64 = 1024 * 1024;

#[derive(Parser)]
#[command(name = CONTAINER_RUNTIME_NAME, version = CONTAINER_RUNTIME_VERSION)]
struct Args {
    /// Path to the Unix-domain socket
    /// on which the work node runtime listens for CRI requests from the Kubelet.
    /// This should probably be '/run/vimana/workd.sock'.
    incoming: String,

    /// Path to the Unix-domain socket
    /// to which the requests for OCI pods and images are forwarded.
    downstream: String,

    /// URL base of the container registry (scheme, host, optional port)
    /// for non-OCI containers.
    registry: String,

    /// Maximum size (in bytes, approximate) of the local in-memory cache
    /// for compiled containers.
    #[arg(short, long, default_value_t = DEFAULT_CONTAINER_CACHE_MAX_CAPACITY)]
    container_cache_max_capacity: u64,
}

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn StdError>> {
    let args = Args::parse();

    let log_processor =
        BatchLogProcessor::builder(StdoutLogExporter::default(), TokioOtelRuntime).build();
    let logger_provider = LoggerProvider::builder()
        .with_log_processor(log_processor)
        .build();
    set_boxed_logger(Box::new(OpenTelemetryLogBridge::new(&logger_provider)))
        .expect("Error setting up logger");

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

    // systemd sends SIGTERM to stop services, CTRL+C sends SIGINT.
    // Listen for those to shut down the servers gracefully.
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

    let runtime = Arc::new(WorkRuntime::new(
        args.registry,
        args.container_cache_max_capacity,
        oci_runtime_client,
        oci_image_client,
        shutdown_rx.shared(),
    )?);

    // Bind to our CRI API socket.
    // This is last thing before starting the servers (with shutdown)
    // because any failures that occur after this should cause the socket to be unlinked
    // so the service can be restarted successfully.
    create_dir_all(Path::new(&args.incoming).parent().unwrap())?;
    let cri_listener = UnixListener::bind(&args.incoming)
        .expect(&format!("Cannot bind Unix socket '{}'", &args.incoming));

    let result = Server::builder()
        .add_service(RuntimeServiceServer::new(VimanaCriService(runtime.clone())))
        .add_service(ImageServiceServer::new(VimanaCriService(runtime)))
        .serve_with_incoming_shutdown(UnixListenerStream::new(cri_listener), shutdown_signal)
        .await;

    // Remove the UDS path after shutdown so we can rebind on restart.
    // Do this before propagating potential CRI API server errors.
    let unlink_socket_result = remove_file(&args.incoming);

    result?;
    Ok(unlink_socket_result?)
}
