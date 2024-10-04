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

use http_body::Body as HttpBody;
use tonic::codec::{
    CompressionEncoding, Decoder, EnabledCompressionEncodings, ProstCodec, Streaming,
};
use tonic::server::Grpc;
use tower_service::Service;

use error::{Error, Result};
use names::FullVersionName;
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

// TODO: Maybe we can get rid of the `dyn`?
type BoxFuture<T, E> = Pin<Box<dyn Future<Output = StdResult<T, E>> + Send + 'static>>;

/// Wrapper around [ActioRuntime] that can implement [RuntimeService] and [ImageService]
/// without running afoul of Rust's rules on foreign types / traits.
pub struct ActioDataPlaneServer(pub Arc<WorkRuntime>);

impl<B> Service<http::Request<B>> for ActioDataPlaneServer
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
        // Extract the domain, version, service name, and RPC name
        // from the request headers.
        let headers = request.headers();
        let domain = headers
            .get(AUTHORITY_HEADER)
            .ok_or(Error::leaf("Expected authority header"))?;
        let version = headers
            .get(VERSION_HEADER)
            .ok_or(Error::leaf("Expected version header"))?;
        // The path header looks like `/ServiceName/RpcName`.
        let path = headers
            .get(PATH_HEADER)
            .ok_or(Error::leaf("Expected path header"))?
            .split('/')
            .skip(1); // Drop the leading slash.
        let service = path.next().ok_or(Error::leaf("Path header malformed"))?;
        let rpc = path.next().ok_or(Error::leaf("Path header malformed"))?;
        if unlikely(path.next().is_some()) {
            return Err(Error::leaf("Path header malformed"));
        }
        let name = FullVersionName::new(domain, service, version)?;

        let encoding = CompressionEncoding::from_encoding_header(
            headers,
            EnabledCompressionEncodings::default(),
        )?;

        let (parts, body) = request.into_parts();

        let mut stream = pin!(Streaming::new_request(
            self.codec.decoder(),
            body,
            encoding,
            None
        ));

        //struct VersionSvc<T: RuntimeService>(pub Arc<T>);
        //impl<T: RuntimeService> tonic::server::UnaryService<super::VersionRequest> for VersionSvc<T> {
        //    type Response = super::VersionResponse;
        //    type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        //    fn call(&mut self, request: tonic::Request<super::VersionRequest>) -> Self::Future {
        //        let inner = Arc::clone(&self.0);
        //        let fut = async move { <T as RuntimeService>::version(&inner, request).await };
        //        Box::pin(fut)
        //    }
        //}
        let inner = self.inner.clone();
        let fut = async move {
            let svc = VersionSvc(inner);
            let mut grpc = Grpc::new(ProstCodec::default());
            let res = grpc.unary(svc, request).await;
            Ok(res)
        };
        Box::pin(fut)
        // _ => Box::pin(async move {
        //     Ok(http::Response::builder()
        //         .status(200)
        //         .header("grpc-status", tonic::Code::Unimplemented as i32)
        //         .header(
        //             http::header::CONTENT_TYPE,
        //             tonic::metadata::GRPC_CONTENT_TYPE,
        //         )
        //         .body(empty_body())
        //         .unwrap())
        // }),
    }
}
