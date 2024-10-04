/// Entrypoint to the work node controller.
/// A single instance of this binary runs in each work node.
use std::env;
use std::error::Error;
use std::sync::Arc;

use api_proto::runtime::v1::image_service_server::ImageServiceServer;
use api_proto::runtime::v1::runtime_service_server::RuntimeServiceServer;
use cri::ActioCriService;
use state::WorkRuntime;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

/// Name of the environment variable
/// holding the path to the Unix-domain socket
/// on which the Work Node runtime listens for CRI requests from the Kubelet.
const CRI_SOCKET_KEY: &str = "CRI_SOCKET";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Arc::new(WorkRuntime::new()?);

    // Listen to the configured Unix Domain Socket.
    let cri_socket = env::var(CRI_SOCKET_KEY)?;
    let cri_listener = UnixListener::bind(&cri_socket)
        .unwrap_or_else(|err| panic!("Cannot bind Unix socket '{cri_socket}': {err}"));

    tokio::try_join!(
        // Serve both the Runtime Service and the Image Service from a single UDS.
        Server::builder()
            .add_service(RuntimeServiceServer::new(ActioCriService(runtime.clone())))
            .add_service(ImageServiceServer::new(ActioCriService(runtime)))
            .serve_with_incoming(UnixListenerStream::new(cri_listener)),
    )?;

    Ok(())
}
