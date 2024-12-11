/// General server boilerplate for all data-plane services.
use std::error::Error;
use std::net::{IpAddr, SocketAddr};

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::{Body, Frame};
use hyper::server::conn::http1::Builder as Http1Builder;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::tokio::TokioIo;
use tokio::net::TcpListener;
use tower_service::Service;

/// TODO: Serve data-plane traffic on an external IP address.
const ADDRESS: [u8; 4] = [127, 0, 0, 1];

/// Serve data-plane traffic on the default HTTPS port number.
/// This is a UDP port for HTTP/3.
const PORT: u16 = 443;

/// This is our service handler. It receives a Request, routes on its
/// path, and returns a Future of a Response.
async fn serve(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::http::Error> {
    let builder = Response::builder();

    // Everything in gRPC is POST.
    debug_assert!(req.method() == Method::POST);
    let uri = req.uri();

    // Ingress should ensure requests are valid.
    // Regular gRPC traffic always uses `200` for the HTTP status code,
    // so using anything else indicates a platform issue, not user error.
    return match uri.host() {
        Some(domain) => {
            let path: &str = uri.path();
            // The path should have the form: "/" <component-name> "/" <method-name>
            // Ignore the first slash and extract the component and method names.
            debug_assert!(path.starts_with('/'));
            let mut path_parts = path[1..].split('/');
            match path_parts.next() {
                Some(component_name) => match path_parts.next() {
                    Some(method_name) => {
                        debug_assert!(path_parts.next().is_none());
                        // The component name should have the form: <service-name> "@" <version>
                        // Extract the service name and version.
                        let mut component_parts = component_name.split('@');
                        match component_parts.next() {
                            Some(service_name) => match component_parts.next() {
                                Some(version) => {
                                    debug_assert!(component_parts.next().is_none());
                                    let config =
                                        get_config(domain, service_name, version, method_name);
                                    bad_request(b"TODO")
                                }
                                None => bad_request(b"Missing version in URI"),
                            },
                            None => bad_request(b"Missing service name in URI"),
                        }
                    }
                    None => bad_request(b"Missing method name in URI"),
                },
                None => bad_request(b"Missing service name and version in URI"),
            }
        }
        None => bad_request(b"Missing host in URI"),
    };
}

async fn get_config(domain: &str, service_name: &str, version: &str, method_name: &str) {}

fn bad_request(
    msg: &'static [u8],
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::http::Error> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(full(msg))
}

fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}

pub async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let addr = SocketAddr::new(IpAddr::from(ADDRESS), PORT);
    let listener = TcpListener::bind(addr).await?;
    println!("Listening on https://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);

        tokio::task::spawn(async move {
            if let Err(err) = Http1Builder::new()
                .serve_connection(io, service_fn(serve))
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

impl<T, B> Service<http::Request<B>> for RuntimeServiceServer<T>
where
    T: RuntimeService,
    B: Body + std::marker::Send + 'static,
    B::Error: Into<StdError> + std::marker::Send + 'static,
{
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        match req.uri().path() {
            "/runtime.v1.RuntimeService/Version" => {
                #[allow(non_camel_case_types)]
                struct VersionSvc<T: RuntimeService>(pub Arc<T>);
                impl<T: RuntimeService> tonic::server::UnaryService<super::VersionRequest> for VersionSvc<T> {
                    type Response = super::VersionResponse;
                    type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                    fn call(
                        &mut self,
                        request: tonic::Request<super::VersionRequest>,
                    ) -> Self::Future {
                        let inner = Arc::clone(&self.0);
                        let fut =
                            async move { <T as RuntimeService>::version(&inner, request).await };
                        Box::pin(fut)
                    }
                }
                let accept_compression_encodings = self.accept_compression_encodings;
                let send_compression_encodings = self.send_compression_encodings;
                let max_decoding_message_size = self.max_decoding_message_size;
                let max_encoding_message_size = self.max_encoding_message_size;
                let inner = self.inner.clone();
                let fut = async move {
                    let method = VersionSvc(inner);
                    let codec = tonic::codec::ProstCodec::default();
                    let mut grpc = tonic::server::Grpc::new(codec)
                        .apply_compression_config(
                            accept_compression_encodings,
                            send_compression_encodings,
                        )
                        .apply_max_message_size_config(
                            max_decoding_message_size,
                            max_encoding_message_size,
                        );
                    let res = grpc.unary(method, req).await;
                    Ok(res)
                };
                Box::pin(fut)
            }
            "/runtime.v1.RuntimeService/RunPodSandbox" => {
                #[allow(non_camel_case_types)]
                struct RunPodSandboxSvc<T: RuntimeService>(pub Arc<T>);
                impl<T: RuntimeService> tonic::server::UnaryService<super::RunPodSandboxRequest>
                    for RunPodSandboxSvc<T>
                {
                    type Response = super::RunPodSandboxResponse;
                    type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
                    fn call(
                        &mut self,
                        request: tonic::Request<super::RunPodSandboxRequest>,
                    ) -> Self::Future {
                        let inner = Arc::clone(&self.0);
                        let fut = async move {
                            <T as RuntimeService>::run_pod_sandbox(&inner, request).await
                        };
                        Box::pin(fut)
                    }
                }
                let accept_compression_encodings = self.accept_compression_encodings;
                let send_compression_encodings = self.send_compression_encodings;
                let max_decoding_message_size = self.max_decoding_message_size;
                let max_encoding_message_size = self.max_encoding_message_size;
                let inner = self.inner.clone();
                let fut = async move {
                    let method = RunPodSandboxSvc(inner);
                    let codec = tonic::codec::ProstCodec::default();
                    let mut grpc = tonic::server::Grpc::new(codec)
                        .apply_compression_config(
                            accept_compression_encodings,
                            send_compression_encodings,
                        )
                        .apply_max_message_size_config(
                            max_decoding_message_size,
                            max_encoding_message_size,
                        );
                    let res = grpc.unary(method, req).await;
                    Ok(res)
                };
                Box::pin(fut)
            }
            _ => Box::pin(async move {
                Ok(http::Response::builder()
                    .status(200)
                    .header("grpc-status", tonic::Code::Unimplemented as i32)
                    .header(
                        http::header::CONTENT_TYPE,
                        tonic::metadata::GRPC_CONTENT_TYPE,
                    )
                    .body(empty_body())
                    .unwrap())
            }),
        }
    }
}
