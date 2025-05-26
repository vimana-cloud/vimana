//! General server boilerplate for all data-plane services.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::result::Result as StdResult;
use std::sync::Arc;

use anyhow::{anyhow, Context, Error, Result};
use axum::body::Body as AxumBody;
use axum::routing::method_routing::post;
use futures::future::Shared;
use futures::FutureExt;
use http::{Request as HttpRequest, Response as HttpResponse};
use tokio::task::spawn;
use tonic::body::BoxBody;
use tonic::codec::{Codec as TonicCodec, EnabledCompressionEncodings};
use tonic::metadata::KeyAndValueRef;
use tonic::server::{Grpc, UnaryService};
use tonic::service::Routes;
use tonic::{Request as TonicRequest, Response as TonicResponse, Status};
use wasmtime::component::{ComponentExportIndex, InstancePre, Val};
use wasmtime::{Engine as WasmEngine, Store};

use crate::containers::ContainerStore;
use crate::host::{grpc_linker, HostState};
use crate::state::SingleUse;
use decode::RequestDecoder;
use encode::ResponseEncoder;
use logging::log_warn;
use metadata_proto::work::runtime::Field;
use names::ComponentName;

/// gRPC pods always use this arbitrarily chosen port for networking.
pub(crate) const GRPC_PORT: u16 = 80;

/// Initializes pods in the background.
///
/// Unlike regular asynchronous functions,
/// returned futures are [`Shared`], so they can be polled by multiple threads,
/// and work begins immediately without having to poll them.
pub(crate) struct PodInitializer {
    /// Means to fetch containers from an external registry.
    containers: ContainerStore,
}

/// Pod initialization starts asynchronously during `RunPodSandbox`,
/// then may be completed by another thread during `StartContainer`,
/// so it must use a [`Shared`] future.
///
/// Make the error [`SingleUse`] so it satisfies the [`Clone`] requirement
/// and can be converted to an unwrapped [`Error`] later.
/// Ideally, this would be something `anyhow` could support natively instead:
/// https://github.com/dtolnay/anyhow/issues/405.
pub(crate) type SharedResultFuture<T> =
    Shared<Pin<Box<dyn Future<Output = StdResult<Arc<T>, SingleUse<Error>>> + Send>>>;

impl PodInitializer {
    pub(crate) fn new(containers: ContainerStore) -> Self {
        PodInitializer { containers }
    }

    /// Initialize a new gRPC pod for the named component using a background task.
    /// A gRPC pod is represented by a Tonic [`Routes`] object that implements it.
    pub(crate) fn grpc(
        &self,
        wasmtime: &WasmEngine,
        name: Arc<ComponentName>,
    ) -> SharedResultFuture<Routes> {
        spawn(initialize_grpc(
            wasmtime.clone(),
            self.containers.clone(),
            name.clone(),
        ))
        .map(|result| {
            result
                .context("Failure joining initialize-gRPC background task")
                .map_err(SingleUse::of)?
                .context("Failure initializing gRPC pod")
                .map_err(SingleUse::of)
        })
        .boxed()
        .shared()
    }
}

/// Initialize a new gRPC pod for the named component.
async fn initialize_grpc(
    wasmtime: WasmEngine,
    containers: ContainerStore,
    name: Arc<ComponentName>,
) -> StdResult<Arc<Routes>, Error> {
    let container = containers.get(name.as_ref()).await?;
    let metadata = (&container.metadata.grpc)
        .as_ref()
        .ok_or(anyhow!("gRPC metadata is empty"))?;

    let linker = grpc_linker(&wasmtime)?;
    let instantiator = linker
        .instantiate_pre(&container.component)
        .context("Linking error")?;

    let mut method_router = Routes::default().into_axum_router();
    for (method_name, method) in metadata.methods.iter() {
        let codec = Codec::new(
            method
                .request
                .as_ref()
                .ok_or(anyhow!("Metadata missing request"))?,
            method
                .response
                .as_ref()
                .ok_or(anyhow!("Metadata missing response"))?,
            name.clone(),
        )?;

        let (_, export_index) = container
            .component
            .export_index(None, &method.function)
            .ok_or_else(|| anyhow!("Function not found: {:?}", method.function))?;

        let method = Method(Arc::new(MethodInner {
            function: export_index,
            instantiator: instantiator.clone(),
            wasmtime: wasmtime.clone(),
            state: Arc::new(HostState::new()),
            component: name.clone(),
        }));

        method_router = method_router.route(
            &format!("/{}", method_name),
            post(|request: HttpRequest<AxumBody>| {
                Box::pin(async move {
                    // Codec and method objects are cloned here.
                    let codec = codec;
                    let method = method;

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

    Ok(Arc::new(Routes::from(
        Routes::default()
            .into_axum_router()
            .nest(&format!("/{}", metadata.service), method_router),
    )))
}

// TODO: Revisit these limits.
/// Maximum request size is 1MiB.
const MAX_DECODING_MESSAGE_SIZE: Option<usize> = Some(1024 * 1024);
/// Maximum response size is 1MiB.
const MAX_ENCODING_MESSAGE_SIZE: Option<usize> = Some(1024 * 1024);

/// Implements Tonic's [`Codec`](TonicCodec)
/// to convert serialized requests/responses to/from Wasm [`Val`] objects.
/// See also [`CodecInner`].
///
/// Reference-counted because it's cloned every the method's handling function is invoked.
#[derive(Clone)]
pub(crate) struct Codec(Arc<CodecInner>);

/// A message decoder (for requests) and an encoder (for responses).
struct CodecInner {
    decoder: RequestDecoder,
    encoder: ResponseEncoder,
}

/// Pairs with a [`Codec`] to implement a service (*e.g.* [`UnaryService`])
/// where the requests and responses are [component values](Val).
///
/// Reference-counted because it's cloned every the method's handling function is invoked.
#[derive(Clone)]
struct Method(Arc<MethodInner>);

/// See [`Method`].
struct MethodInner {
    /// Index of the function in the component to handle this RPC.
    function: ComponentExportIndex,

    /// An efficient means of instantiating new instances.
    instantiator: InstancePre<Arc<HostState>>,

    /// Global Wasm engine to run hosted services.
    wasmtime: WasmEngine,

    /// Shared host state used by every method in this pod.
    state: Arc<HostState>,

    /// Name of the component this method is a part of, for error logging.
    component: Arc<ComponentName>,
}

impl Codec {
    pub(crate) fn new(
        decoder: &Field,
        encoder: &Field,
        component: Arc<ComponentName>,
    ) -> Result<Self> {
        Ok(Codec(Arc::new(CodecInner {
            decoder: RequestDecoder::new(decoder, component.clone())?,
            encoder: ResponseEncoder::new(encoder, component)?,
        })))
    }
}

impl TonicCodec for Codec {
    type Encode = Val;
    type Decode = Val;
    type Encoder = ResponseEncoder;
    type Decoder = RequestDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        self.0.encoder.clone()
    }

    fn decoder(&mut self) -> Self::Decoder {
        self.0.decoder.clone()
    }
}

type BoxedStatusResultFuture<T> =
    Pin<Box<dyn Future<Output = StdResult<T, Status>> + Send + 'static>>;

impl UnaryService<Val> for Method {
    type Response = Val;
    type Future = BoxedStatusResultFuture<TonicResponse<Self::Response>>;

    fn call(&mut self, request: TonicRequest<Val>) -> Self::Future {
        let method = self.clone();
        Box::pin(async move {
            // TODO: See if we can pool instances somehow.
            let mut store = Store::new(&method.0.wasmtime, method.0.state.clone());
            let instance = method
                .0
                .instantiator
                .instantiate_async(&mut store)
                .await
                .map_err(|error| {
                    // TODO: Log these errors.
                    let _component = method.0.component.as_ref();
                    Status::internal("Module instantiation error")
                })?;

            let function = instance
                .get_func(&mut store, &method.0.function)
                .ok_or_else(|| {
                    // TODO: Log these errors.
                    let _function_index = &method.0.function;
                    Status::internal("Function selection error")
                })?;

            let (metadata, extensions, request) = request.into_parts();

            let mut headers = Vec::with_capacity(metadata.len());
            for header in metadata.iter() {
                match header {
                    KeyAndValueRef::Ascii(key, value) => {
                        if let Ok(value) = value.to_str() {
                            let key = String::from(key.as_str());
                            let value = String::from(value);
                            headers.push(Val::Tuple(vec![Val::String(key), Val::String(value)]));
                        } else {
                            // Silently ignore non-ASCII header value, but log a warning.
                            log_warn!(
                                component: method.0.component.as_ref(),
                                "Non-ASCII request header value: {:?} = {:?}",
                                key, value,
                            );
                        }
                    }
                    KeyAndValueRef::Binary(key, value) => {
                        // Silently ignore non-ASCII header key, but log a warning.
                        log_warn!(
                            component: method.0.component.as_ref(),
                            "Non-ASCII request header: {:?} = {:?}",
                            key, value,
                        );
                    }
                }
            }

            let context = Val::Record(vec![("headers".into(), Val::List(headers))]);
            let parameters = vec![context, request];

            // The results slice just has to have the right size.
            // Contents are ignored and overridden during invocation.
            let mut results = vec![Val::Option(None)];

            function
                .call_async(&mut store, &parameters, &mut results)
                .await
                .map_err(|error| {
                    // TODO: Log these errors.
                    let _component = method.0.component.as_ref();
                    Status::internal("Function invocation error")
                })?;

            let response = TonicResponse::new(
                // Should be safe to pop since we initialized it with an item.
                results.pop().unwrap(),
            );
            Ok(response)
        })
    }
}
