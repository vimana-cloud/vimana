//! State machine used by the CRI service to manage pods.
#![feature(async_closure)]

use std::collections::HashMap;
use std::error::Error as StdError;
use std::mem::drop;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as SyncMutex;

use papaya::{Compute, HashMap as LockFreeConcurrentHashMap, Operation};
use tokio::sync::oneshot;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::{spawn, AbortHandle};
use tonic::service::Routes;
use tonic::transport::channel::Channel;
use tonic::transport::server::TcpIncoming;
use tonic::transport::Server;
use wasmtime::{Config as WasmConfig, Engine as WasmEngine};

use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::PodSandboxMetadata;
use error::{Error, Result};
use grpc_pod::{PodFuture, PodInitializer};
use names::{CanonicalComponentName, PodId};

/// Global runtime state for a work node.
pub struct WorkRuntime {
    /// Global Wasm engine to run hosted services.
    /// This is a cheap, thread-safe handle to the "real" engine.
    wasmtime: WasmEngine,

    /// Map of locally running pod IDs to pod controllers.
    /// Lock-freedom is important to help isolate tenants from one another.
    pods: LockFreeConcurrentHashMap<PodId, PodController>,

    /// To generate unique pod IDs.
    next_pod_id: AtomicUsize,

    /// Remote store from which to retrieve container images by ID,
    /// which can then be loaded into pods.
    pod_store: PodInitializer,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    pub oci_runtime: AsyncMutex<RuntimeServiceClient<Channel>>,

    /// Client to the downstream OCI CRI image service.
    pub oci_image: AsyncMutex<ImageServiceClient<Channel>>,
}

/// Pod lifecycle states.
enum PodController {
    /// A pod after initialization with `RunPodSandbox` but before `CreateContainer`.
    Initiated(InitiatedPodInfo),

    /// A pod after creating the container with `CreateContainer` but before `StartContainer`.
    Created(CreatedPodInfo),

    /// A pod after starting the container with `StartContainer`.
    Running(RunningPodInfo),

    /// After calling `StopContainer` but before removal.
    Stopped,

    /// After calling `RemoveContainer` but before `RemovePodSandbox`.
    Removed,
}

/// Info configured by `RunPodSandbox`.
#[derive(Clone)]
struct InitiatedPodInfo {
    /// K8s metadata. Must be returned as-is for status requests.
    metadata: PodSandboxMetadata,

    /// Canonical name of the component that will run in this pod.
    component_name: CanonicalComponentName,

    /// Port that the pod should listen on for the gRPC server.
    grpc_port: u16,

    /// Compile component and metadata (future).
    /// Starts initializing as soon as possible.
    pod: PodFuture,
}

/// Info configured by `CreateContainer`.
#[derive(Clone)]
struct CreatedPodInfo {
    /// Info configured by `RunPodSandbox`.
    initial: InitiatedPodInfo,

    /// Environment variable keys and values.
    environment: HashMap<String, String>,
}

/// Info configured by `StartContainer`.
struct RunningPodInfo {
    /// Info configured by `CreateContainer`.
    created: CreatedPodInfo,

    /// Send to this channel to shut down the server gracefully.
    shutdown: SyncMutex<Option<oneshot::Sender<()>>>,

    // Use this to shut down the server forcibly.
    abort: AbortHandle,
}

impl WorkRuntime {
    /// Return a new runtime with no running pods.
    pub async fn new(
        registry: String,
        oci_runtime: RuntimeServiceClient<Channel>,
        oci_image: ImageServiceClient<Channel>,
    ) -> Result<Self> {
        let wasmtime = Self::default_engine()?;
        let pod_store = PodInitializer::new(registry, &wasmtime);
        Ok(Self {
            wasmtime,
            pods: LockFreeConcurrentHashMap::new(),
            next_pod_id: AtomicUsize::new(0),
            pod_store,
            oci_runtime: AsyncMutex::new(oci_runtime),
            oci_image: AsyncMutex::new(oci_image),
        })
    }

    // A new instance of the default engine for this runtime.
    fn default_engine() -> Result<WasmEngine> {
        WasmEngine::new(
            WasmConfig::new()
                // Allow host functions to be `async` Rust.
                // Means you have to use `Func::call_async` instead of `Func::call`.
                .async_support(true)
                // Epoch interruption for preemptive multithreading.
                // https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html#method.epoch_interruption
                .epoch_interruption(true)
                // Enable support for various Wasm proposals...
                .wasm_component_model(true)
                .wasm_gc(true)
                .wasm_tail_call(true)
                .wasm_function_references(true),
        )
        .map_err(|err| Error::wrap("Error allocating engine", err))
    }

    /// Create a new [pod controller](PodController)
    /// in the [initiated](PodController::Initiated) state.
    /// Return a newly generated ID.
    ///
    /// A pod does not serve traffic until the container is [created](Self::create_container)
    /// and then [started](Self::start_container) therein.
    pub fn init_pod(
        &self,
        metadata: PodSandboxMetadata,
        component_name: CanonicalComponentName,
        grpc_port: u16,
    ) -> Result<PodId> {
        let pod = self.pod_store.initialize(&self.wasmtime, &component_name);
        let controller = PodController::Initiated(InitiatedPodInfo {
            metadata,
            component_name,
            grpc_port,
            pod,
        });

        let pod_id = self.next_pod_id.fetch_add(1, Ordering::Relaxed);
        let pods = self.pods.pin();
        match pods.try_insert(pod_id, controller) {
            Ok(_) => Ok(pod_id),
            Err(_) => Err(Error::leaf(format!("Pod ID '{pod_id}' already in use."))),
        }
    }

    /// Set the environment variables in an [initiated](PodController::Initiated) pod controller,
    /// converting it to a [created](PodController::Created) controller.
    pub fn create_container(
        &self,
        pod_id: PodId,
        environment: HashMap<String, String>,
    ) -> Result<()> {
        let pods = self.pods.pin();
        match pods.compute(pod_id, |entry| match entry {
            Some((_, value)) => match value {
                PodController::Initiated(initial) => {
                    Operation::Insert(PodController::Created(CreatedPodInfo {
                        initial: initial.clone(),
                        environment: environment.clone(),
                    }))
                }
                PodController::Created(created) => {
                    // Support idempotency if the parameters are exactly the same.
                    if created.environment == environment {
                        Operation::Abort(None)
                    } else {
                        Operation::Abort(Some(Error::leaf(format!(
                            "Cannot re-create container '{pod_id}' with different environment",
                        ))))
                    }
                }
                state => Operation::Abort(Some(Error::leaf(format!(
                    "Cannot create container in {} pod '{pod_id}'",
                    state.adjective(),
                )))),
            },
            None => Operation::Abort(Some(Error::leaf(format!("Pod '{pod_id}' not found")))),
        }) {
            Compute::Updated { old: _, new: _ } | Compute::Aborted(None) => Ok(()),
            Compute::Aborted(Some(error)) => Err(error),
            _ => Err(Error::impossible()),
        }
    }

    /// Start up a server for a [created](PodController::Created) pod controller
    /// on its configured gRPC port,
    /// converting it to a [running](PodController::Running) controller.
    pub async fn start_container(&self, pod_id: PodId) -> Result<()> {
        let pods = self.pods.pin();
        // Optimistically try to update the state assuming the pod is finished initializing.
        match pods.compute(pod_id, |entry| match entry {
            Some((_, value)) => match value {
                PodController::Created(created) => match created.initial.pod.peek() {
                    Some(Ok(pod)) => {
                        // It's ready! proceed on the happy path.
                        start_available_pod(pod, &created)
                    }
                    Some(Err(init_error)) => {
                        Operation::Abort(StartContainerAbort::Error(Error::leaf(format!(
                            "Error while initializing pod for '{}': {:?}",
                            created.initial.component_name, init_error,
                        ))))
                    }
                    None => {
                        // Still waiting, so we'll have to await async then try again.
                        Operation::Abort(StartContainerAbort::Waiting(created.initial.pod.clone()))
                    }
                },
                PodController::Running(running) => {
                    // Support idempotency if the parameters are exactly the same.
                    Operation::Abort(StartContainerAbort::Done)
                }
                state => Operation::Abort(StartContainerAbort::Error(Error::leaf(format!(
                    "Cannot start container in {} pod '{pod_id}'",
                    state.adjective(),
                )))),
            },
            None => Operation::Abort(StartContainerAbort::Error(Error::leaf(format!(
                "Pod '{pod_id}' not found"
            )))),
        }) {
            Compute::Updated { old: _, new: _ } | Compute::Aborted(StartContainerAbort::Done) => {
                Ok(())
            }
            Compute::Aborted(StartContainerAbort::Waiting(future)) => {
                // Drop the pin on the pods hashmap, await the future, then try again.
                drop(pods);
                // TODO: Throwing away this result then recursing means cloning the `Routes`
                //       implementing the pod unnecessarily. Try to avoid that.
                let _ = future.await;
                Box::pin(self.start_container(pod_id)).await
            }
            Compute::Aborted(StartContainerAbort::Error(error)) => Err(error),
            _ => Err(Error::impossible()),
        }
    }
}

/// Possible reasons why starting a container might be aborted.
/// See [`start_container`](WorkRuntime::start_container).
enum StartContainerAbort {
    /// Pod is still initializing asynchronously.
    Waiting(PodFuture),
    /// There was a problem.
    Error(Error),
    /// Support idempotency if the pod is already started.
    Done,
}

/// Helper function to simplify [`start_container`](WorkRuntime::start_container).
fn start_available_pod(
    pod: &Routes,
    created: &CreatedPodInfo,
) -> Operation<PodController, StartContainerAbort> {
    let addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        created.initial.grpc_port,
    );
    // TODO: Revisit implications of nodelay.
    let nodelay = true;
    // TODO: Revisit implications of keepalive.
    let keepalive = None;
    // Synchronously bind to the port so errors can be handled immediately.
    TcpIncoming::new(addr, nodelay, keepalive).map_or_else(
        |err| {
            Operation::Abort(StartContainerAbort::Error(Error::leaf(format!(
                "Cannot bind gRPC port '{}'",
                created.initial.grpc_port
            ))))
        },
        |incoming| {
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            // Oneshot receivers return an error when the sender is dropped before sending.
            // The server should shut down either way, so ignore the result.
            let shutdown = async move {
                let _ = shutdown_rx.await;
            };
            let abort = spawn(
                // [This suggestion](https://github.com/hyperium/tonic/pull/1893)
                // using `into_axum_router` obviates the need to implement Tonic's `NamedService`
                // which is not dyn-compatible.
                Server::builder()
                    .add_routes(pod.clone())
                    .serve_with_incoming_shutdown(incoming, shutdown),
            )
            .abort_handle();
            Operation::Insert(PodController::Running(RunningPodInfo {
                created: created.clone(),
                shutdown: SyncMutex::new(Some(shutdown_tx)),
                abort,
            }))
        },
    )
}

const INITIATED_POD_ADJECTIVE: &str = "initiated";
const CREATED_POD_ADJECTIVE: &str = "created";
const RUNNING_POD_ADJECTIVE: &str = "running";
const STOPPED_POD_ADJECTIVE: &str = "stopped";
const REMOVED_POD_ADJECTIVE: &str = "removed";

impl PodController {
    fn adjective(&self) -> &'static str {
        match self {
            PodController::Initiated(_) => INITIATED_POD_ADJECTIVE,
            PodController::Created(_) => CREATED_POD_ADJECTIVE,
            PodController::Running(_) => RUNNING_POD_ADJECTIVE,
            PodController::Stopped => STOPPED_POD_ADJECTIVE,
            PodController::Removed => REMOVED_POD_ADJECTIVE,
        }
    }
}
