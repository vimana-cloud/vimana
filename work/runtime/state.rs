//! State machine used by the CRI service to manage pods.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::result::Result as StdResult;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex as SyncMutex;
use std::time::{Duration, SystemTime};

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

use crate::ipam::{ActiveIpAddress, Ipam};
use crate::pods::{PodInitializer, SharedResultFuture, GRPC_PORT};
use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::{
    ContainerMetadata, PodSandboxMetadata, PodSandboxState, PodSandboxStateValue,
};
use error::{log_error, log_error_status, log_warn, Result};
use names::{ComponentName, PodId, PodName};

/// Global runtime state for a work node.
pub(crate) struct WorkRuntime {
    /// Global Wasm engine to run hosted services.
    /// This is a cheap, thread-safe handle to the "real" engine.
    wasmtime: WasmEngine,

    /// Map of locally running pod IDs to pod controllers.
    /// Lock-freedom is important to help isolate tenants from one another.
    pods: LockFreeConcurrentHashMap<PodId, Pod>,

    /// To generate unique pod IDs.
    next_pod_id: AtomicUsize,

    /// Remote store from which to retrieve container images by ID,
    /// which can then be loaded into pods.
    pod_store: PodInitializer,

    /// IP address management system.
    ipam: Ipam,

    /// Name of the network interface to use (e.g. `eth0`).
    network_interface: String,

    /// All data-place servers should start gracefully shutting down
    /// upon completion of this shareable future.
    /// Individual pods can be shut down with their [killer](ContainerKiller).
    shutdown: Shared<oneshot::Receiver<()>>,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    pub(crate) oci_runtime: AsyncMutex<RuntimeServiceClient<Channel>>,

    /// Client to the downstream OCI CRI image service.
    pub(crate) oci_image: AsyncMutex<ImageServiceClient<Channel>>,
}

/// Pod lifecycle state.
///
/// Pods generally follow a simple linear lifecycle:
///     initiated → created → starting → running → stopped → removed
/// Although, other lifecycles are theoretically possible.
///
/// Each transition typically maps to an RPC in the CRI API.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PodState {
    /// A pod after initialization with `RunPodSandbox` but before `CreateContainer`.
    Initiated,

    /// A pod after creating the container with `CreateContainer` but before `StartContainer`.
    Created,

    /// This trasition occurs immediately at the start of `StartContainer`
    /// to act as a sort of mutex before starting up a background task.
    Starting,

    /// A pod after starting the container with `StartContainer`.
    Running,

    /// After calling `StopContainer` but before removal.
    Stopped,

    /// After calling `RemoveContainer` but before `RemovePodSandbox`.
    Removed,
}

/// All information known about a pod / container pair
/// throughout its [lifecycle](PodState).
#[derive(Clone)]
pub(crate) struct Pod {
    // --------------------------------
    // The following are always populated:
    // --------------------------------
    /// Current state of the pod.
    pub(crate) state: PodState,

    /// Pod IP address.
    pub(crate) ip_address: ActiveIpAddress,

    /// Canonical name of the component that will run in this pod (used for logs).
    pub(crate) component_name: Arc<ComponentName>,

    /// Axum router implementing the pod.
    /// Starts initializing as soon as possible.
    routes: SharedResultFuture<Arc<Routes>>,

    /// K8s metadata. Must be returned as-is for status requests.
    pub(crate) pod_sandbox_metadata: PodSandboxMetadata,

    /// Resource labels associated with the pod sandbox --
    /// and also with the container;
    /// Workd should verify that every container has the same labels as its pod.
    pub(crate) labels: HashMap<String, String>,

    /// Vimana doesn't really use these (yet), but they're here.
    pub(crate) annotations: HashMap<String, String>,

    /// Creation timestamp of the pod sandbox in nanoseconds. Must be > 0.
    pub(crate) pod_created_at: i64,

    // --------------------------------
    // The following are populated after `CreateContainer`:
    // --------------------------------
    /// Creation timestamp of the container in nanoseconds. Must be > 0.
    pub(crate) container_created_at: i64,

    /// K8s metadata. Must be returned as-is for status requests.
    pub(crate) container_metadata: Option<ContainerMetadata>,

    /// Environment variable keys and values.
    environment: HashMap<String, String>,

    // --------------------------------
    // The following are populated after `StartContainer`:
    // --------------------------------
    /// Start timestamp of the container in nanoseconds. Must be > 0.
    pub(crate) container_started_at: i64,

    /// Shuts down the running container, either the easy way or the hard way.
    killer: Option<Arc<ContainerKiller>>,

    // --------------------------------
    // The following are populated after `StopContainer`:
    // --------------------------------
    /// Stop timestamp of the container in nanoseconds. Must be > 0.
    pub(crate) container_finished_at: i64,
}

/// Used to shut down a running container.
struct ContainerKiller {
    /// Send to this channel to shut down the server gracefully.
    shutdown: SyncMutex<Option<oneshot::Sender<()>>>,

    /// Useful for two things:
    /// - Awaiting graceful shutdown after sending the signal to [`shutdown`](Self::shutdown).
    /// - Forcibly shutting down.
    join: JoinHandle<StdResult<(), ServerError>>,
}

impl WorkRuntime {
    /// Return a new runtime with no running pods.
    pub(crate) fn new(
        registry: String,
        max_container_cache_capacity: u64,
        oci_runtime: RuntimeServiceClient<Channel>,
        oci_image: ImageServiceClient<Channel>,
        network_interface: String,
        ipam: Ipam,
        shutdown: Shared<oneshot::Receiver<()>>,
    ) -> StdResult<Self, WasmError> {
        let wasmtime = Self::default_engine()?;
        let pod_store = PodInitializer::new(registry, max_container_cache_capacity, &wasmtime);
        Ok(Self {
            wasmtime,
            pods: LockFreeConcurrentHashMap::new(),
            next_pod_id: AtomicUsize::new(0),
            pod_store,
            ipam,
            network_interface,
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
    pub(crate) async fn init_pod(
        &self,
        component_name: ComponentName,
        pod_sandbox_metadata: PodSandboxMetadata,
        labels: HashMap<String, String>,
        annotations: HashMap<String, String>,
    ) -> Result<PodName> {
        // TODO: Does the pod ID have to be unique within a node, or across all nodes?
        //   if the latter, figure out how to get a unique node ID involved somehow.
        let pod_id = self.next_pod_id.fetch_add(1, Ordering::Relaxed);
        let pod_name = PodName::new(component_name.clone(), pod_id);
        let component_name = Arc::new(component_name);

        // Start initializing the pod immediately in a background task.
        let (routes, abort_routes) = self.pod_store.grpc(&self.wasmtime, component_name.clone());

        // TODO: This blocks. Can it not?
        let ip_address = self
            .ipam
            .address(&pod_name)
            .and_then(|allocated| allocated.add(&self.network_interface))
            .map_err(|error| {
                abort_routes.abort();
                error
            })?;

        let pod = Pod {
            state: PodState::Initiated,
            ip_address,
            component_name,
            routes,
            pod_sandbox_metadata,
            labels,
            annotations,
            pod_created_at: now(),
            // These are set at later states:
            container_created_at: 0,
            container_metadata: None,
            environment: HashMap::new(),
            container_started_at: 0,
            killer: None,
            container_finished_at: 0,
        };

        let pods = self.pods.pin();
        match pods.try_insert(pod_id, pod) {
            Ok(_) => Ok(pod_name),
            Err(_) => {
                // Impossible unless the number of pods overflows `usize`.
                Err(Status::internal("pod-id-in-use"))
            }
        }
    }

    /// Set the environment variables in an [initiated](PodController::Initiated) pod controller,
    /// converting it to a [created](PodController::Created) controller.
    pub(crate) fn create_container(
        &self,
        pod_name: &PodName,
        container_metadata: &Option<ContainerMetadata>,
        environment: &HashMap<String, String>,
    ) -> Result<()> {
        let pods = self.pods.pin();
        match pods.compute(pod_name.pod, |entry| match entry {
            Some((_, pod)) => match &pod.state {
                PodState::Initiated => {
                    let mut pod = pod.clone();
                    pod.state = PodState::Created;
                    pod.container_metadata = container_metadata.clone();
                    pod.environment = environment.clone();
                    pod.container_created_at = now();
                    Operation::Insert(pod)
                }
                PodState::Created => {
                    // Support idempotency if the parameters are exactly the same.
                    if pod.container_metadata == *container_metadata
                        && pod.environment == *environment
                    {
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

    /// Start up a server for a [created](PodState::Created) pod controller
    /// on its configured gRPC port.
    ///
    /// First, convert it to a [starting](PodState::Starting) controller
    /// (to establish mutual exclusivity),
    /// then spawn the background task to run the server,
    /// then convert it to a [running](PodState::Running) controller
    /// (to mark it as complete).
    ///
    /// Upon successful completion, return `Ok(None)`.
    /// If the pod is still initializing, return `Ok(Some(<future>))`.
    /// The caller can await the future, which is cheaply cloneable, before trying again.
    /// This allows us to implement this function non-asynchronously.
    pub(crate) fn start_container(
        &self,
        pod_name: PodName,
    ) -> Result<Option<SharedResultFuture<Arc<Routes>>>> {
        let pods = self.pods.pin();
        // Optimistically try to update the state assuming the pod is finished initializing.
        match pods.compute(pod_name.pod, |entry| match entry {
            Some((_, pod)) => match &pod.state {
                PodState::Created => match pod.routes.peek() {
                    Some(Ok(_)) => {
                        // The server is ready! Now we just have to bind to a socket and start it.
                        // Effectively lock a mutex on it by transitioning to *starting*.
                        // All other paths end in abortion.
                        let mut pod = pod.clone();
                        pod.state = PodState::Starting;
                        Operation::Insert(pod)
                    }
                    Some(Err(init_error)) => {
                        // Propagate any initialization errors up the stack.
                        Operation::Abort(StartContainerAbort::Error(init_error.clone()))
                    }
                    None => {
                        // Still initializing; await the future then retry.
                        Operation::Abort(StartContainerAbort::Waiting(pod.routes.clone()))
                    }
                },
                PodState::Starting | PodState::Running => {
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
                Operation::Abort(StartContainerAbort::Error(Status::not_found(
                    "start-container-pod-not-found",
                )))
            }
        }) {
            Compute::Updated {
                old: _,
                new: (_, pod),
            } => {
                // The only code path that results in `Compute::Updated` transitions to `Starting`
                // and should have verified that the routes are ready and OK,
                // so we should be able to just unwrap it here.
                debug_assert!(pod.state == PodState::Starting);
                let routes = pod.routes.peek().clone().unwrap().clone().unwrap();

                let address = SocketAddr::new(pod.ip_address.allocated.address, GRPC_PORT);
                // TODO: Revisit implications of nodelay.
                let nodelay = true;
                // TODO: Revisit implications of keepalive.
                let keepalive = None;

                // Synchronously bind to the port so errors can be handled immediately.
                TcpIncoming::new(address, nodelay, keepalive).map_or_else(
                    |err| {
                        // "Unlock" the state machine "mutex" by setting the state back to `Created`
                        // before returning any error.
                        let mut pod = pod.clone();
                        pod.state = PodState::Created;
                        pods.insert(pod_name.pod, pod);
                        Err(log_error_status!("bind-grpc-port", &pod_name.component)(
                            err,
                        ))
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
                                .add_routes(routes.as_ref().clone())
                                .serve_with_incoming_shutdown(incoming, shutdown),
                        );

                        let mut pod = pod.clone();
                        pod.state = PodState::Running;
                        pod.killer = Some(Arc::new(ContainerKiller {
                            shutdown: SyncMutex::new(Some(shutdown_target_tx)),
                            join: task,
                        }));
                        pod.container_started_at = now();
                        pods.insert(pod_name.pod, pod);

                        Ok(None)
                    },
                )
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

    /// Like [`Self::list_pods`],
    /// but with the added `name` condition for exact match by ID.
    /// Skips the exhaustive search and adds at most 1 pod to results.
    /// Does nothing of the pod can't be found.
    pub(crate) fn get_pod<T, F>(
        &self,
        name: &PodName,
        state: &Option<PodSandboxStateValue>,
        labels: &Vec<(&String, &String)>,
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        if let Some(controller) = self.pods.pin().get(&name.pod) {
            Self::match_pod(&name.pod, controller, state, labels, transform, results);
        }
    }

    /// List all the pods that match the given state (if provided) and labels.
    /// Push results into the provided vector.
    ///
    /// Currently implemented by searching the pod map exhaustively (*O(n)*).
    /// YAGNIndices?
    pub(crate) fn list_pods<T, F>(
        &self,
        state: &Option<PodSandboxStateValue>,
        labels: &Vec<(&String, &String)>,
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        for (id, controller) in self.pods.pin().iter() {
            Self::match_pod(id, controller, state, labels, transform, results);
        }
    }

    /// Logic common to [`Self::get_pod`] and [`Self::list_pods`].
    ///
    /// Check if the given pod (represented by `pod_id` and `pod`)
    /// matches the given filter conditions (`state` and `labels`).
    /// If so, convert it to a CRI API [`PodSandbox`]
    /// and append it to `results`
    /// after passing it through `transform`.
    #[inline(always)]
    fn match_pod<T, F>(
        pod_id: &PodId,
        pod: &Pod,
        state: &Option<PodSandboxStateValue>,
        labels: &Vec<(&String, &String)>,
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        // If state is unspecified, or specifically 'ready',
        // then the state filter always passes because pods are always ready
        // (containers might not be).
        // Otherwise, the whole filter always fails
        // because conditions are composed with AND.
        if state.map_or(true, |state_value| {
            state_value.state == PodSandboxState::SandboxReady as i32
        }) && labels.iter().all(|(key, value)| {
            // Look up each key, which must be present,
            // and check that the value matches as well.
            pod.labels
                .get(*key)
                .map_or(false, |actual| actual == *value)
        }) {
            // This clone is not strictly necessary
            // (it effectively means double-cloning the service name).
            // We just need something `Display` that behaves like a `PodName`.
            let name = PodName::new(pod.component_name.as_ref().clone(), *pod_id);
            results.push(transform(&name, pod));
        }
    }
}

/// Possible reasons why starting a container might be aborted.
/// See [`start_container`](WorkRuntime::start_container).
enum StartContainerAbort {
    /// Pod is still initializing asynchronously.
    Waiting(SharedResultFuture<Arc<Routes>>),
    /// There was a problem.
    Error(Status),
    /// Support idempotency if the pod is already started.
    Done,
}

// Return non-leap nanoseconds since 1970-01-01 00:00:00 UTC+0 as `i64`.
// Return zero if executed before 1970.
// May wrap around in 2262.
pub(crate) fn now() -> i64 {
    (SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64
        % (i64::MAX as u64)) as i64
}
