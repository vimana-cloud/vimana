//! General server boilerplate for all data-plane services.
#![feature(core_intrinsics)]

use core::pin::pin;
use std::error::Error as StdError;
use std::future::Future;
use std::intrinsics::unlikely;
use std::pin::Pin;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::header::{HeaderMap, HeaderValue};
use http_body::Body as HttpBody;
use tonic::codec::{
    CompressionEncoding, Decoder, EnabledCompressionEncodings, ProstCodec, Streaming,
};
use tonic::server::Grpc;
use tower_service::Service;

use error::{Error, Result};
use names::ComponentName;
use state::WorkRuntime;

/// These are standard gRPC headers:
/// https://github.com/grpc/grpc/blob/v1.66.1/doc/PROTOCOL-HTTP2.md#requests.
/// `:authority` always indicates a service's domain.
/// `:path` always contains the service and method names.
const AUTHORITY_HEADER: &str = ":authority";
const PATH_HEADER: &str = ":path";
/// This one is non-standard. It indicates the service version.
/// It should always be explicitly set by ingress.
const VERSION_HEADER: &str = ":version";

// TODO: Maybe we can get rid of the `dyn` by picking a concrete Future type.
type BoxFuture<T, E> = Pin<Box<dyn Future<Output = StdResult<T, E>> + Send + 'static>>;

/// Wrapper around [WorkRuntime] that can implement [RuntimeService] and [ImageService]
/// without running afoul of Rust's rules on foreign types / traits.
pub struct DataPlaneServer(pub Arc<WorkRuntime>);

impl<B> Service<http::Request<B>> for DataPlaneServer
where
    B: HttpBody + Send + 'static,
    //B::Error: Into<StdError> + Send + 'static,
    B::Error: Send + 'static,
{
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<StdResult<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        Box::pin(async move {
            let headers = request.headers();
            let (component_name, rpc_name) = extract_version_and_rpc_names(headers)?;

            let encoding = CompressionEncoding::from_encoding_header(
                headers,
                EnabledCompressionEncodings::default(),
            )?;

            self.0.run_on_pod(component_name, async move {});

            let (head, body) = request.into_parts();

            //let mut stream = pin!(Streaming::new_request(
            //    self.codec.decoder(),
            //    body,
            //    encoding,
            //    None
            //));

            //let inner = self.inner.clone();
            //let fut = async move {
            //    let svc = VersionSvc(inner);
            //    let mut grpc = Grpc::new(ProstCodec::default());
            //    let res = grpc.unary(svc, request).await;
            //    Ok(res)
            //};
        })
    }
}

/// Extract the full version name (domain, service name, and version),
/// and RPC name, from request headers.
#[inline(always)]
fn extract_version_and_rpc_names(
    headers: &HeaderMap<HeaderValue>,
) -> Result<(ComponentName, String)> {
    let domain = headers
        .get(AUTHORITY_HEADER)
        .ok_or(Error::leaf("Expected authority header"))?
        .to_str()?;
    let version = headers
        .get(VERSION_HEADER)
        .ok_or(Error::leaf("Expected version header"))?
        .to_str()?;
    let path = headers
        .get(PATH_HEADER)
        .ok_or(Error::leaf("Expected path header"))?
        .to_str()?;

    // Parse the path header, which looks like `/ServiceName/RpcName`.
    // It must have at least 4 bytes (two slashes, and non-empty service / RPC names).
    if unlikely(path.len() < 4 || path.as_bytes()[0] != b'/') {
        return Err(Error::leaf("Path header malformed"));
    }
    let mut path_parts = path[1..].split('/');
    let service = path_parts
        .next()
        .ok_or(Error::leaf("Path header malformed"))?;
    let rpc = path_parts
        .next()
        .ok_or(Error::leaf("Path header malformed"))?;
    if unlikely(path_parts.next().is_some()) {
        return Err(Error::leaf("Path header malformed"));
    }

    Ok((
        ComponentName::new(domain, service, version)?,
        String::from(rpc),
    ))
}
