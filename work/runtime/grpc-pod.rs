//! General server boilerplate for all data-plane services.

use std::convert::{identity, Infallible};
use std::future::Future;
use std::pin::Pin;
use std::result::Result as StdResult;
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::routing::method_routing::post;
use futures::future::Shared;
use futures::FutureExt;
use http::{Request as HttpRequest, Response as HttpResponse};
use tokio::task::spawn;
use tonic::body::BoxBody;
use tonic::codec::{
    Codec as TonicCodec, DecodeBuf, Decoder as TonicDecoder, EnabledCompressionEncodings,
    EncodeBuf, Encoder as TonicEncoder,
};
use tonic::server::{Grpc, UnaryService};
use tonic::service::Routes;
use tonic::{Code, Request as TonicRequest, Response as TonicResponse, Status};
use wasmtime::component::{ComponentExportIndex, InstancePre, Linker, Val};
use wasmtime::{Engine as WasmEngine, Store};

use containers::ContainerStore;
use decode::{Decoder, MessageDecoder};
use encode::{Encoder, MessageEncoder};
use error::Result;
use grpc_container_proto::work::runtime::grpc_metadata::method::Field;
use names::CanonicalComponentName;

// TODO: Revisit these limits.
/// Maximum request size is 1MiB.
const MAX_DECODING_MESSAGE_SIZE: Option<usize> = Some(1024 * 1024);
/// Maximum response size is 1MiB.
const MAX_ENCODING_MESSAGE_SIZE: Option<usize> = Some(1024 * 1024);

/// State available to host-defined functions.
pub type HostState = ();

/// A gRPC pod is represented by a Tonic [`Routes`] object that implements it.
/// It's initialized asynchronously starting during the CRI `RunPodSandbox` event,
/// then may be completed by another thread during `StartContainer`,
/// so it must use a [`Shared`] future.
pub type PodFuture = Shared<Pin<Box<dyn Future<Output = StdResult<Routes, PodInitError>> + Send>>>;

/// The shareable [`PodFuture`] requires a cloneable error type.
#[derive(Clone, Debug)]
pub enum PodInitError {
    /// Unrecognized component name.
    NotFound,

    /// Problem compiling the component from bytecode.
    CompileError,

    /// Problem resolving imports for the component.
    LinkError,

    /// There is an issue with the container metadata
    /// (anything besides the Wasm byte code).
    InvalidMetadata,

    /// The background task used to implement [`initialize`](PodInitializer::initialize)
    /// either panicked or was cancelled. This should hopefully never happen.
    TaskJoinError,
}

/// Initializes pods in the background.
pub struct PodInitializer {
    /// Means to fetch containers from an external registry.
    containers: Arc<ContainerStore>,
}

/// Cheaply cloneable codec that can implement
/// Tonic's [`Codec`](TonicCodec), [`Decoder`](TonicDecoder), and [`Encoder`](TonicEncoder)
/// to convert serialized requests/responses to/from Wasm [`Val`] objects.
/// See also [`CodecInner`].
#[derive(Clone)]
pub struct Codec(Arc<CodecInner>);

/// A message decoder (for requests) and an encoder (for responses).
struct CodecInner {
    decoder: MessageDecoder,
    encoder: MessageEncoder,
}

/// Cheaply cloneable object used to pass a Wasm [`Val`]
/// into the given function of a given component.
#[derive(Clone)]
pub struct Method {
    /// Index of the function in the component to handle this RPC.
    pub function: ComponentExportIndex,

    /// An efficient means of instantiating new instances.
    pub instantiator: InstancePre<HostState>,

    /// Global Wasm engine to run hosted services.
    pub wasmtime: WasmEngine,
}

impl PodInitializer {
    pub fn new(registry: String, wasmtime: &WasmEngine) -> Self {
        PodInitializer {
            containers: Arc::new(ContainerStore::new(registry, wasmtime)),
        }
    }

    /// Initialize a new pod for the named component using a background task.
    /// Unlike a regular asynchronous function,
    /// the returned future is [`Shared`] so it can potentially be polled by multiple threads,
    /// and work begins immediately without having to poll it.
    pub fn initialize(&self, wasmtime: &WasmEngine, name: &CanonicalComponentName) -> PodFuture {
        // Complete all work in a background task so it can proceed without polling.
        spawn(initialize_pod(
            wasmtime.clone(),
            self.containers.clone(),
            name.clone(),
        ))
        // Only potential join errors have to be handled in the foreground.
        .map(|recv_result| recv_result.map_or(Err(PodInitError::TaskJoinError), identity))
        .boxed()
        .shared()
    }
}

/// Initialize a new pod for the named component.
async fn initialize_pod(
    wasmtime: WasmEngine,
    containers: Arc<ContainerStore>,
    name: CanonicalComponentName,
) -> StdResult<Routes, PodInitError> {
    let (component, metadata) = containers.get(name).await.map_err(|_| todo!())?;

    let linker = Linker::new(&wasmtime);

    let instantiator = linker
        .instantiate_pre(&component)
        .map_err(|_| PodInitError::LinkError)?;

    let mut method_router = Routes::default().into_axum_router();
    for (method_name, method) in metadata.methods.iter() {
        let codec = Codec::from_protos(
            method
                .request
                .as_ref()
                .ok_or(PodInitError::InvalidMetadata)?,
            method
                .response
                .as_ref()
                .ok_or(PodInitError::InvalidMetadata)?,
        )
        .map_err(|_| PodInitError::InvalidMetadata)?;

        let (_, export_index) = component
            .export_index(None, &method.function_name)
            .ok_or(PodInitError::InvalidMetadata)?;

        let method = Method {
            function: export_index,
            instantiator: instantiator.clone(),
            wasmtime: wasmtime.clone(),
        };

        method_router = method_router.route(
            &format!("/{}", method_name),
            post(|request: HttpRequest<AxumBody>| {
                Box::pin(async move {
                    let mut grpc = Grpc::new(codec)
                        .apply_compression_config(
                            EnabledCompressionEncodings::default(),
                            EnabledCompressionEncodings::default(),
                        )
                        .apply_max_message_size_config(
                            MAX_DECODING_MESSAGE_SIZE,
                            MAX_ENCODING_MESSAGE_SIZE,
                        );
                    // TODO: Handle streaming RPC's (currently assumes all are unary).
                    Ok::<HttpResponse<BoxBody>, Infallible>(grpc.unary(method, request).await)
                })
            }),
        );
    }

    Ok(Routes::from(Routes::default().into_axum_router().nest(
        &format!("/{}", metadata.service_name),
        method_router,
    )))
}

impl Codec {
    pub fn from_protos(decoder: &Field, encoder: &Field) -> Result<Self> {
        Ok(Codec(Arc::new(CodecInner {
            decoder: MessageDecoder::new(decoder)?,
            encoder: MessageEncoder::new(encoder)?,
        })))
    }
}

impl TonicCodec for Codec {
    type Encode = Val;
    type Decode = Val;
    type Encoder = Codec;
    type Decoder = Codec;

    fn encoder(&mut self) -> Self::Encoder {
        self.clone()
    }

    fn decoder(&mut self) -> Self::Decoder {
        self.clone()
    }
}

impl TonicDecoder for Codec {
    type Item = Val;
    type Error = Status;

    /// Decode a message from a buffer containing exactly the bytes of a full message.
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Self::Item>, Self::Error> {
        self.0.decoder.decode(src)
    }
}

impl TonicEncoder for Codec {
    type Item = Val;
    type Error = Status;

    /// Encode a message to a writable buffer.
    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> StdResult<(), Self::Error> {
        self.0.encoder.encode(item, dst)
    }
}

type BoxFuture<T, E> = Pin<Box<dyn Future<Output = StdResult<T, E>> + Send + 'static>>;

impl UnaryService<Val> for Method {
    type Response = Val;
    type Future = BoxFuture<TonicResponse<Self::Response>, Status>;

    fn call(&mut self, request: TonicRequest<Val>) -> Self::Future {
        let method = self.clone();
        Box::pin(async move {
            // TODO: See if we can pool instances somehow.
            let mut store = Store::new(&method.wasmtime, ());
            let instance = method
                .instantiator
                .instantiate_async(&mut store)
                .await
                .map_err(|_| Status::new(Code::Internal, "Failed to instantiate"))?;
            let function = instance
                .get_func(&mut store, &method.function)
                .unwrap_or_else(|| todo!());

            // TODO: Something with metadata and extensions.
            let (metadata, extensions, request) = request.into_parts();
            let parameters = vec![request];
            // The results slice just has to have the right size.
            // Values are ignored and overridden during invocation.
            let mut results = vec![Val::Bool(false)];

            function
                .call_async(&mut store, &parameters, &mut results)
                .await
                .map_err(|_| Status::new(Code::Internal, "Failed to invoke function"))?;

            let response = TonicResponse::new(results.pop().unwrap_or_else(|| todo!()));
            Ok(response)
        })
    }
}
