/// Entrypoint to the work node controller.
/// A single instance of this binary runs in each work node.
use std::env::args;
use std::error::Error as StdError;
use std::fs::{create_dir_all, remove_file};
use std::path::Path;
use std::result::Result as StdResult;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use tokio::net::{UnixListener, UnixStream};
use tokio::select;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Endpoint, Server, Uri};
use tower::service_fn;

use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::image_service_server::ImageServiceServer;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::runtime_service_server::RuntimeServiceServer;
use cri::{VimanaCriService, CONTAINER_RUNTIME_VERSION};
use state::WorkRuntime;

/// Path to the Unix-domain socket
/// on which the work node runtime listens for CRI requests from the Kubelet.
const CRI_SOCKET: &str = "/run/vimana/workd.sock";

#[tokio::main]
async fn main() -> StdResult<(), Box<dyn StdError>> {
    // Parse command-line arguments by hand because they're so simple.
    // It takes exactly 1 argument:
    // the path to a UDS for a downstream CRI server (such as containerd)
    // to which to proxy all requests for running OCI-compatible pods,
    // or `--version` to just print the version and exit.
    let mut args = args();
    let name = args.next();
    let argument = args.next().unwrap_or_else(|| {
        panic!(
            "Usage: {} ( <path> | --version )",
            name.unwrap_or(String::from("workd"))
        )
    });
    if argument == "--version" {
        println!("{}", CONTAINER_RUNTIME_VERSION);
        return Ok(());
    }

    // This seems to be the most idiomatic way to create a client with a UDS transport:
    // https://github.com/hyperium/tonic/blob/v0.12.3/examples/src/uds/client.rs.
    // The socket path must be cloneable to enable re-invoking the connector function.
    let oci_socket = Arc::new(argument);
    let channel = Endpoint::from_static("http://unused")
        .connect_with_connector(service_fn(move |_: Uri| {
            let oci_socket = oci_socket.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(
                    UnixStream::connect(oci_socket.as_ref()).await?,
                ))
            }
        }))
        .await?;
    let oci_image_client = ImageServiceClient::new(channel.clone());
    let oci_runtime_client = RuntimeServiceClient::new(channel);

    let runtime = Arc::new(WorkRuntime::new(oci_runtime_client, oci_image_client).await?);

    // Bind to our CRI API socket.
    // This is last thing before starting the servers (with shutdown)
    // because any failures that occur after this should cause the socket to be unlinked
    // so the service can be restarted successfully.
    create_dir_all(Path::new(CRI_SOCKET).parent().unwrap())?;
    let cri_listener = UnixListener::bind(CRI_SOCKET)
        .unwrap_or_else(|err| panic!("Cannot bind Unix socket '{CRI_SOCKET}': {err}"));

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

    Server::builder()
        .add_service(RuntimeServiceServer::new(VimanaCriService(runtime.clone())))
        .add_service(ImageServiceServer::new(VimanaCriService(runtime)))
        .serve_with_incoming_shutdown(UnixListenerStream::new(cri_listener), shutdown_signal)
        .await?;

    // Remove the UDS path after shutdown so we can rebind on restart.
    remove_file(CRI_SOCKET)?;

    Ok(())
}
