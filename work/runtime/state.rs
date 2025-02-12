//! State machine used by the CRI service to manage pods.
#![feature(async_closure)]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::result::Result as StdResult;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex as SyncMutex;

use futures::future::Shared;
use papaya::{Compute, HashMap as LockFreeConcurrentHashMap, Operation};
use tokio::select;
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::task::{spawn, JoinHandle};
use tonic::service::Routes;
use tonic::transport::channel::Channel;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Error as ServerError, Server};
use tonic::Status;
use wasmtime::{Config as WasmConfig, Engine as WasmEngine, Error as WasmError};

use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::PodSandboxMetadata;
use error::{log_error, log_error_status, log_warn, Result};
use grpc_pod::{PodFuture, PodInitializer};
use names::{ComponentName, PodId, PodName};

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

    /// Data-place servers should start gracefully shutting down
    /// upon completion of this shareable future.
    shutdown: Shared<oneshot::Receiver<()>>,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    pub oci_runtime: AsyncMutex<RuntimeServiceClient<Channel>>,

    /// Client to the downstream OCI CRI image service.
    pub oci_image: AsyncMutex<ImageServiceClient<Channel>>,
}

/// Pod lifecycle state machine.
///
/// Pods generally follow a simple linear lifecycle:
///     *initiated* → *created* → *starting* → *running* → *stopped* → *removed*
#[derive(Debug)]
enum PodController {
    /// A pod after initialization with `RunPodSandbox` but before `CreateContainer`.
    Initiated(InitiatedPodInfo),

    /// A pod after creating the container with `CreateContainer` but before `StartContainer`.
    Created(CreatedPodInfo),

    /// This trasition occurs immediately at the start of `StartContainer`
    /// to act as a sort of mutex before starting up a background task.
    Starting(StartingPodInfo),

    /// A pod after starting the container with `StartContainer`.
    Running(RunningPodInfo),

    /// After calling `StopContainer` but before removal.
    Stopped,

    /// After calling `RemoveContainer` but before `RemovePodSandbox`.
    Removed,
}

/// Info configured by `RunPodSandbox`.
#[derive(Clone, Debug)]
struct InitiatedPodInfo {
    /// K8s metadata. Must be returned as-is for status requests.
    metadata: PodSandboxMetadata,

    /// Canonical name of the component that will run in this pod.
    component_name: Arc<ComponentName>,

    /// Port that the pod should listen on for the gRPC server.
    grpc_port: u16,

    /// Compile component and metadata (future).
    /// Starts initializing as soon as possible.
    pod: PodFuture<Arc<Routes>>,
}

/// Info configured by `CreateContainer`.
#[derive(Clone, Debug)]
struct CreatedPodInfo {
    /// Info configured by `RunPodSandbox`.
    initial: InitiatedPodInfo,

    /// Environment variable keys and values.
    environment: HashMap<String, String>,
}

/// Info configured by `CreateContainer`.
#[derive(Clone, Debug)]
struct StartingPodInfo {
    /// Info configured by `CreateContainer`.
    created: CreatedPodInfo,

    /// Ready-to-run server.
    pod: Arc<Routes>,
}

/// Info configured by `StartContainer`.
#[derive(Debug)]
struct RunningPodInfo {
    /// Info configured by `CreateContainer`.
    created: CreatedPodInfo,

    /// Send to this channel to shut down the server gracefully.
    shutdown: SyncMutex<Option<oneshot::Sender<()>>>,

    // Useful for two things:
    // - Awaiting graceful shutdown after sending the signal to [`shutdown`](Self::shutdown).
    // - Forcibly shutting down.
    join: JoinHandle<StdResult<(), ServerError>>,
}

impl WorkRuntime {
    /// Return a new runtime with no running pods.
    pub fn new(
        registry: String,
        max_container_cache_capacity: u64,
        oci_runtime: RuntimeServiceClient<Channel>,
        oci_image: ImageServiceClient<Channel>,
        shutdown: Shared<oneshot::Receiver<()>>,
    ) -> StdResult<Self, WasmError> {
        let wasmtime = Self::default_engine()?;
        let pod_store = PodInitializer::new(registry, max_container_cache_capacity, &wasmtime);
        Ok(Self {
            wasmtime,
            pods: LockFreeConcurrentHashMap::new(),
            next_pod_id: AtomicUsize::new(0),
            pod_store,
            shutdown,
            oci_runtime: AsyncMutex::new(oci_runtime),
            oci_image: AsyncMutex::new(oci_image),
        })
    }

    // A new instance of the default engine for this runtime.
    fn default_engine() -> StdResult<WasmEngine, WasmError> {
        WasmEngine::new(
            WasmConfig::new()
                // Allow host functions to be `async` Rust.
                // Means you have to use `Func::call_async` instead of `Func::call`.
                .async_support(true)
                // Epoch interruption for preemptive multithreading.
                // https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html#method.epoch_interruption
                //.epoch_interruption(true)
                // Enable support for various Wasm proposals...
                .wasm_component_model(true)
                .wasm_gc(true)
                .wasm_tail_call(true)
                .wasm_function_references(true),
        )
    }

    /// Create a new [pod controller](PodController)
    /// in the [initiated](PodController::Initiated) state.
    /// Return a newly generated ID.
    ///
    /// A pod does not serve gRPC traffic until the container is [created](Self::create_container)
    /// and then [started](Self::start_container) therein.
    pub fn init_pod(
        &self,
        component_name: ComponentName,
        metadata: PodSandboxMetadata,
        grpc_port: u16,
    ) -> Result<PodId> {
        let component_name = Arc::new(component_name);
        let pod = self.pod_store.grpc(&self.wasmtime, component_name.clone());
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
            Err(_) => {
                // Logically impossible
                // unless the number of number of pods overflows `usize`.
                Err(Status::internal("pod-id-in-use"))
            }
        }
    }

    /// Set the environment variables in an [initiated](PodController::Initiated) pod controller,
    /// converting it to a [created](PodController::Created) controller.
    pub fn create_container(
        &self,
        pod_name: PodName,
        environment: HashMap<String, String>,
    ) -> Result<()> {
        let pods = self.pods.pin();
        match pods.compute(pod_name.pod, |entry| match entry {
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
                        // Cannot re-create the container with a different environment.
                        Operation::Abort(Some(Status::failed_precondition(
                            "container-already-created",
                        )))
                    }
                }
                state => {
                    log_error!("create-container-bad-state", &pod_name.component, state);
                    Operation::Abort(Some(Status::failed_precondition(
                        "create-container-bad-state",
                    )))
                }
            },
            None => Operation::Abort(Some(Status::failed_precondition(
                "create-container-pod-not-found",
            ))),
        }) {
            Compute::Updated { old: _, new: _ } | Compute::Aborted(None) => Ok(()),
            Compute::Aborted(Some(error)) => Err(error),
            _ => {
                // Logically impossible
                // (all possible compute outcomes are handled).
                Err(Status::internal("impossible"))
            }
        }
    }

    /// Start up a server for a [created](PodController::Created) pod controller
    /// on its configured gRPC port.
    ///
    /// First, convert it to a [starting](PodController::Starting) controller
    /// (to establish mutual exclusion),
    /// then spawn the background task to run the server,
    /// then convert it to a [running](PodController::Running) controller
    /// (to mark it as complete).
    pub fn start_container(&self, pod_name: PodName) -> Result<Option<PodFuture<Arc<Routes>>>> {
        let pods = self.pods.pin();
        // Optimistically try to update the state assuming the pod is finished initializing.
        match pods.compute(pod_name.pod, |entry| match entry {
            Some((_, value)) => match value {
                PodController::Created(created) => match created.initial.pod.peek() {
                    Some(Ok(pod)) => {
                        // It's finished initializing!
                        // Establish mutual exclusion over it by transitioning to *starting*.
                        // All other paths end in abortion.
                        Operation::Insert(PodController::Starting(StartingPodInfo {
                            created: created.clone(),
                            pod: pod.clone(),
                        }))
                    }
                    Some(Err(init_error)) => {
                        // Propagate any initialization errors up the stack.
                        Operation::Abort(StartContainerAbort::Error(init_error.clone()))
                    }
                    None => {
                        // Still initializing; await the future then retry.
                        Operation::Abort(StartContainerAbort::Waiting(created.initial.pod.clone()))
                    }
                },
                PodController::Starting(_) | PodController::Running(_) => {
                    // Support idempotency.
                    Operation::Abort(StartContainerAbort::Done)
                }
                state => {
                    // Unexpected Kubelet behavior.
                    log_error!("start-container-bad-state", &pod_name.component, state);
                    Operation::Abort(StartContainerAbort::Error(Status::failed_precondition(
                        "start-container-bad-state",
                    )))
                }
            },
            None => {
                // Unexpected Kubelet behavior.
                Operation::Abort(StartContainerAbort::Error(Status::failed_precondition(
                    "start-container-pod-not-found",
                )))
            }
        }) {
            Compute::Updated {
                old: _,
                new: (_, controller),
            } => {
                if let PodController::Starting(StartingPodInfo { created, pod }) = controller {
                    // Always listen on address 127.0.0.1.
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
                            // "Unlock" the state machine mutex before returning an error.
                            pods.insert(pod_name.pod, PodController::Created(created.clone()));
                            Err(log_error_status!("bid-grpc-port", &pod_name.component)(err))
                        },
                        |incoming| {
                            // Shut down the server gracefully when either:
                            // - The pod is specifically targetted for shut down by the CRI controller.
                            // - All pods are shut down globally.
                            let (shutdown_target_tx, shutdown_target_rx) = oneshot::channel();
                            let shutdown_global_rx = self.shutdown.clone();
                            let shutdown = async move {
                                select! {
                                    _ = shutdown_target_rx => {}
                                    _ = shutdown_global_rx => {}
                                }
                            };

                            let task = spawn(
                                // [This suggestion](https://github.com/hyperium/tonic/pull/1893)
                                // using `into_axum_router` obviates the need to implement Tonic's `NamedService`
                                // which is not dyn-compatible.
                                Server::builder()
                                    .add_routes(pod.as_ref().clone())
                                    .serve_with_incoming_shutdown(incoming, shutdown),
                            );
                            let state = PodController::Running(RunningPodInfo {
                                created: created.clone(),
                                shutdown: SyncMutex::new(Some(shutdown_target_tx)),
                                join: task,
                            });

                            pods.insert(pod_name.pod, state);
                            Ok(None)
                        },
                    )
                } else {
                    // Logically impossible
                    // (the only compute  path that updates anything inserts `PodController::Starting`).
                    Err(Status::internal("start-container-bad-mutex"))
                }
            }
            Compute::Aborted(StartContainerAbort::Done) => {
                // The pod was already started by some other thread.
                // This seems super unlikely, but I *think* idempotency is desirable here.
                log_warn!("start-container-already", &pod_name.component, ());
                Ok(None)
            }
            Compute::Aborted(StartContainerAbort::Waiting(future)) => {
                // If the future is not yet completed,
                // return it to the caller so they can await it (then try again).
                Ok(Some(future))
            }
            Compute::Aborted(StartContainerAbort::Error(error)) => Err(error),
            _ => {
                // Logically impossible
                // (all possible compute outcomes are handled).
                Err(Status::internal("impossible"))
            }
        }
    }
}

/// Possible reasons why starting a container might be aborted.
/// See [`start_container`](WorkRuntime::start_container).
enum StartContainerAbort {
    /// Pod is still initializing asynchronously.
    Waiting(PodFuture<Arc<Routes>>),
    /// There was a problem.
    Error(Status),
    /// Support idempotency if the pod is already started.
    Done,
}
