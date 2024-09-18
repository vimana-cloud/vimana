/// General server boilerplate for all data-plane services.
use tower_service::Service;

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
