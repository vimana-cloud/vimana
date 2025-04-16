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
use tokio::time::timeout;
use tonic::service::Routes;
use tonic::transport::channel::Channel;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Error as ServerError, Server};
use tonic::Status;
use wasmtime::{Config as WasmConfig, Engine as WasmEngine, Error as WasmError};

use crate::ipam::{IpAddress, Ipam};
use crate::pods::{PodInitializer, SharedResultFuture, GRPC_PORT};
use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::{ContainerMetadata, PodSandboxMetadata};
use error::{log_error, log_error_status, log_info, log_warn, Result};
use names::{ComponentName, PodId, PodName};

const VIMANA_LABEL_PREFIX: &str = "vimana.host/";

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
    /// Individual pods can be shut down with their [killer](Pod::killer).
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
///     initiated → created → starting → running → stopped
/// Although, other lifecycles are theoretically possible,
/// and most transitions must be idempotent.
///
/// Each transition typically maps to an RPC in the CRI API.
/// See [`README.md`](README.md) for more details.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum PodState {
    /// A pod after initialization with `RunPodSandbox` but before `CreateContainer`.
    Initiated,

    /// A pod after creating the container with `CreateContainer` but before `StartContainer`.
    Created,

    /// This trasition occurs immediately at the start of `StartContainer`
    /// to act as a sort of mutex before starting up a background task.
    Starting,

    /// A pod after starting the container with `StartContainer`.
    /// The pod is reachable by on the data plane while in this state.
    Running,

    /// After `StopContainer` but before `RemoveContainer`.
    Stopped,

    /// After `RemoveContainer` but before `StopPodSandbox`.
    Removed,

    /// After `StopPodSandbox` but before `RemovePodSandbox`.
    Killed,
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
    pub(crate) ip_address: IpAddress,

    /// Canonical name of the component that will run in this pod (used for logs).
    pub(crate) component_name: Arc<ComponentName>,

    /// Axum router implementing the pod.
    /// Starts initializing as soon as possible.
    routes: SharedResultFuture<Arc<Routes>>,

    /// K8s metadata. Must be returned as-is for status requests.
    pub(crate) pod_sandbox_metadata: PodSandboxMetadata,

    /// K8s labels associated with the pod sandbox.
    pub(crate) pod_labels: HashMap<String, String>,

    /// K8s annotations associated with the pod sandbox.
    pub(crate) pod_annotations: HashMap<String, String>,

    /// Creation timestamp of the pod sandbox in nanoseconds. Must be > 0.
    pub(crate) pod_created_at: i64,

    // --------------------------------
    // The following are populated after `CreateContainer`:
    // --------------------------------
    /// Creation timestamp of the container in nanoseconds. Must be > 0.
    pub(crate) container_created_at: i64,

    /// K8s metadata. Must be returned as-is for status requests.
    pub(crate) container_metadata: Option<ContainerMetadata>,

    /// K8s labels associated with the container.
    pub(crate) container_labels: HashMap<String, String>,

    /// K8s annotations associated with the container.
    pub(crate) container_annotations: HashMap<String, String>,

    /// Environment variable keys and values.
    environment: HashMap<String, String>,

    // --------------------------------
    // The following are populated after `StartContainer`:
    // --------------------------------
    /// Start timestamp of the container in nanoseconds. Must be > 0.
    pub(crate) container_started_at: i64,

    /// Shuts down the running container, either the easy way or the hard way.
    killer: SingleUse<ContainerKiller>,

    // --------------------------------
    // The following are populated after `StopContainer`:
    // --------------------------------
    /// Stop timestamp of the container in nanoseconds. Must be > 0.
    pub(crate) container_finished_at: i64,
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

        let ip_address = self
            .ipam
            .address(&pod_name, &self.network_interface)
            .await
            .map_err(|error| {
                // TODO:
                //   Does aborting here also abort clones
                //   (like pre-fetching due to `PullImage`)? Is that what we want?
                abort_routes.abort();
                error
            })?;

        let pod = Pod {
            state: PodState::Initiated,
            ip_address,
            component_name,
            routes,
            pod_sandbox_metadata,
            pod_labels: labels,
            pod_annotations: annotations,
            pod_created_at: now(),
            // These are set at later states:
            container_created_at: 0,
            container_metadata: None,
            container_labels: HashMap::default(),
            container_annotations: HashMap::default(),
            environment: HashMap::default(),
            container_started_at: 0,
            killer: SingleUse::default(),
            container_finished_at: 0,
        };

        let pods = self.pods.pin();
        match pods.try_insert(pod_id, pod) {
            Ok(_) => Ok(pod_name),
            Err(_) => {
                // Impossible unless the number of pods overflows `usize`.
                Err(Status::internal("pod-id-collision"))
            }
        }
    }

    /// Set the environment variables in an [initiated](PodController::Initiated) pod controller,
    /// converting it to a [created](PodController::Created) controller.
    pub(crate) fn create_container(
        &self,
        name: &PodName,
        container_metadata: &Option<ContainerMetadata>,
        labels: &HashMap<String, String>,
        annotations: &HashMap<String, String>,
        environment: &HashMap<String, String>,
    ) -> Result<()> {
        let pods = self.pods.pin();
        match pods.compute(name.pod, |entry| match entry {
            Some((_, pod)) => match pod.state {
                PodState::Initiated | PodState::Removed => {
                    // Make sure all the labels that begin with `vimana.host/`
                    // are the same between the pod labels and container labels.
                    //if let Some(error) =
                    //    check_vimana_labels(labels, &pod.pod_labels, "extra-container-labels", name)
                    //{
                    //    Operation::Abort(Some(error))
                    //} else if let Some(error) =
                    //    check_vimana_labels(&pod.pod_labels, labels, "extra-pod-labels", name)
                    //{
                    //    Operation::Abort(Some(error))
                    //} else {
                    // The Vimana labels match. Transition to `Created`.
                    let mut pod = pod.clone();
                    pod.state = PodState::Created;
                    pod.container_metadata = container_metadata.clone();
                    pod.container_labels = labels.clone();
                    pod.container_annotations = annotations.clone();
                    pod.environment = environment.clone();
                    pod.container_created_at = now();
                    Operation::Insert(pod)
                    //}
                }
                PodState::Created => {
                    // Support idempotency if the parameters are exactly the same.
                    if pod.container_metadata == *container_metadata
                        && pod.container_labels == *labels
                        && pod.container_annotations == *annotations
                        && pod.environment == *environment
                    {
                        log_info!("create-container-idempotent", &name.component, pod.state);
                        Operation::Abort(None)
                    } else {
                        // Cannot re-create the container with a different environment.
                        Operation::Abort(Some(log_error_status!(
                            "container-already-created",
                            &name.component
                        )(())))
                    }
                }
                PodState::Starting | PodState::Running | PodState::Stopped | PodState::Killed => {
                    Operation::Abort(Some(log_error_status!(
                        "create-container-bad-state",
                        &name.component
                    )(pod.state)))
                }
            },
            None => Operation::Abort(Some(log_error_status!(
                "create-container-not-found",
                &name.component
            )(()))),
        }) {
            Compute::Updated { old: _, new: _ } => {
                log_info!("create-container-success", &name.component, name.pod);
                Ok(())
            }
            Compute::Aborted(None) => Ok(()),
            Compute::Aborted(Some(error)) => Err(error),
            _ => {
                // Logically impossible (all possible compute outcomes are handled).
                Err(Status::internal("create-container-impossible"))
            }
        }
    }

    /// Start up a server for a [created](PodState::Created) pod controller
    /// on its configured gRPC port.
    ///
    /// First, convert it to a [starting](PodState::Starting) controller
    /// (to establish exclusivity),
    /// then spawn the background task to run the server,
    /// then convert it to a [running](PodState::Running) controller
    /// (to mark it as complete).
    pub(crate) async fn start_container(&self, name: &PodName) -> Result<()> {
        if let Some(future) = self.start_container_without_wait(name)? {
            // Indicates the server was not yet ready. Await it before trying again.
            let _ = future.await;
            if self.start_container_without_wait(name)?.is_some() {
                // This should never happen because we already know the server was ready.
                return Err(log_error_status!(
                    "start-container-impossible-unready",
                    &name.component
                )(()));
            }
        }
        Ok(())
    }

    /// This function exists to sidestep a [known issue][1] with Rust's `Send`-safety detection.
    /// It's non-async so the compiler doesn't worry about the pod map guard being un-`Send`.
    /// Otherwise, [`start_container`](Self::start_container) could have simply recursed.
    ///
    /// [1]: https://users.rust-lang.org/t/future-is-not-send-as-this-value-is-used-across-an-await-but-i-drop-the-value-before-the-await/57574
    fn start_container_without_wait(
        &self,
        name: &PodName,
    ) -> Result<Option<SharedResultFuture<Arc<Routes>>>> {
        let pods = self.pods.pin();
        match pods.compute(name.pod, |entry| match entry {
            Some((_, pod)) => match pod.state {
                PodState::Created | PodState::Stopped => match pod.routes.peek() {
                    Some(Ok(_)) => {
                        // The server is ready! Now just bind to a socket and start it.
                        // Claim responsibility for doing so by transitioning to *starting*.
                        // All other paths end in abortion.
                        let mut pod = pod.clone();
                        pod.state = PodState::Starting;
                        Operation::Insert(pod)
                    }
                    Some(Err(init_error)) => {
                        // Propagate any initialization errors up the stack.
                        // It should have already been logged where it first occurred.
                        Operation::Abort(StartContainerAbort::Error(init_error.clone()))
                    }
                    None => {
                        // Still initializing; await the future then retry.
                        log_info!("starting-container-waiting", &name.component, name.pod);
                        Operation::Abort(StartContainerAbort::Waiting(pod.routes.clone()))
                    }
                },
                PodState::Starting | PodState::Running => {
                    log_info!("starting-container-idempotent", &name.component, name.pod);
                    Operation::Abort(StartContainerAbort::Done)
                }
                PodState::Initiated | PodState::Removed | PodState::Killed => {
                    // Unexpected Kubelet behavior.
                    Operation::Abort(StartContainerAbort::Error(log_error_status!(
                        "starting-container-bad-state",
                        &name.component
                    )(pod.state)))
                }
            },
            None => {
                // Unexpected Kubelet behavior.
                Operation::Abort(StartContainerAbort::Error(log_error_status!(
                    "starting-container-not-found",
                    &name.component
                )(())))
            }
        }) {
            Compute::Updated {
                old: _,
                new: (_, pod),
            } => {
                log_info!("starting-container-success", &name.component, name.pod);

                // The only code path that results in `Compute::Updated`
                // should have verified that the routes are ready and OK.
                let routes = pod
                    .routes
                    .peek()
                    .clone()
                    .ok_or(Status::internal("start-container-impossible"))?
                    .clone()?;
                let address = SocketAddr::new(pod.ip_address.address, GRPC_PORT);
                // TODO: Revisit implications of nodelay.
                let nodelay = true;
                // TODO: Revisit implications of keepalive.
                let keepalive = None;

                TcpIncoming::new(address, nodelay, keepalive).map_or_else(
                    |bind_error| {
                        // If the pod is still `Starting`,
                        // "unlock" its state by setting it back to `Created`
                        // before propagating the bind error.
                        pods.compute(name.pod, |entry| match entry {
                            Some((_, existing_pod)) => match &existing_pod.state {
                                PodState::Starting => {
                                    let mut pod = existing_pod.clone();
                                    pod.state = PodState::Created;
                                    Operation::Insert(pod)
                                }
                                // The pod may have been stopped or killed by another task.
                                // Leave it that way.
                                PodState::Stopped | PodState::Killed => Operation::Abort(()),
                                // These transitions would be unexpected logic errors.
                                // Leave it that way anyway.
                                PodState::Initiated
                                | PodState::Created
                                | PodState::Running
                                | PodState::Removed => {
                                    log_error!(
                                        "started-container-bind-error-bad-state",
                                        &name.component,
                                        &existing_pod.state
                                    );
                                    Operation::Abort(())
                                }
                            },
                            None => {
                                log_error!(
                                    "started-container-bind-error-not-found",
                                    &name.component,
                                    ()
                                );
                                Operation::Abort(())
                            }
                        });
                        Err(log_error_status!("bind-grpc-port", &name.component)(
                            bind_error,
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
                            // [This suggestion](https://github.com/hyperium/tonic/pull/1893),
                            // (using Axum directly instead of Tonic)
                            // obviates the need to implement Tonic's `NamedService`
                            // which is not dyn-compatible.
                            Server::builder()
                                .add_routes(routes.as_ref().clone())
                                .serve_with_incoming_shutdown(incoming, shutdown),
                        );

                        let mut pod = pod.clone();
                        pod.state = PodState::Running;
                        pod.killer = SingleUse::of(ContainerKiller {
                            shutdown: shutdown_target_tx,
                            join: task,
                        });
                        pod.container_started_at = now();

                        // Now update the pod map again,
                        // making sure this pod's state has not changed since we set it to `Starting`.
                        // That would indicate that the "mutex" did not function properly.
                        match pods.compute(name.pod, |entry| match entry {
                            Some((_, existing_pod)) => match &existing_pod.state {
                                PodState::Starting => Operation::Insert(pod.clone()),
                                PodState::Initiated
                                | PodState::Created
                                | PodState::Running
                                | PodState::Stopped
                                | PodState::Removed
                                | PodState::Killed => Operation::Abort(log_error_status!(
                                    "started-container-bad-state",
                                    &name.component
                                )(
                                    &existing_pod.state
                                )),
                            },
                            None => Operation::Abort(log_error_status!(
                                "started-container-not-found",
                                &name.component
                            )(())),
                        }) {
                            Compute::Updated { old: _, new: _ } => {
                                log_info!("started-container-success", &name.component, name.pod);
                                Ok(None)
                            }
                            Compute::Aborted(error) => {
                                // If there was some sort of synchronization error,
                                // kill the server; it shouldn't have received any traffic yet.
                                pod.killer.take().map(ContainerKiller::forcefully_abort);
                                Err(error)
                            }
                            _ => {
                                // Logically impossible (all possible compute outcomes are handled).
                                // Might as well clean up if this happens, though.
                                pod.killer.take().map(ContainerKiller::forcefully_abort);
                                Err(Status::internal("started-container-impossible"))
                            }
                        }
                    },
                )
            }
            Compute::Aborted(StartContainerAbort::Done) => Ok(None),
            Compute::Aborted(StartContainerAbort::Waiting(future)) => Ok(Some(future)),
            Compute::Aborted(StartContainerAbort::Error(error)) => Err(error),
            _ => {
                // Logically impossible (all possible compute outcomes are handled).
                Err(Status::internal("starting-container-impossible"))
            }
        }
    }

    /// Stop a running container by killing the running server
    /// and transitioning the state to [`ContainerStopped`](PodState::ContainerStopped).
    /// Attempts graceful server shutdown at first,
    /// waiting at most `timeout` before forcefully aborting.
    pub(crate) async fn stop_container(&self, name: &PodName, timeout: Duration) -> Result<()> {
        if let Some(killer) = self.stop_container_without_wait(name)? {
            if !killer.kill_with_timeout(timeout).await {
                log_warn!(
                    "stop-container-stopped-forcefully",
                    &name.component,
                    name.pod
                );
            }
        }
        Ok(())
    }

    /// See [`stop_container`](Self::stop_container).
    ///
    /// Similar to [`start_container_without_wait`](Self::start_container_without_wait),
    /// This function only exists to implement the state change synchronously.
    ///
    /// Returns the container's [killer](ContainerKiller)
    /// if the container was previously in [running](PodState::Running)
    /// and had not yet been killed.
    /// Returns `None` otherwise (such as if `StopContainer` was invoked twice).
    fn stop_container_without_wait(&self, name: &PodName) -> Result<Option<ContainerKiller>> {
        let mut prior_state = PodState::Running;
        let pods = self.pods.pin();
        match pods.compute(name.pod, |entry| match entry {
            Some((_, pod)) => match pod.state {
                PodState::Starting | PodState::Running => {
                    let mut pod = pod.clone();
                    prior_state = pod.state;
                    pod.state = PodState::Stopped;
                    Operation::Insert(pod)
                }
                PodState::Stopped => {
                    log_info!("stop-container-idempotent", &name.component, pod.state);
                    Operation::Abort(None)
                }
                PodState::Initiated | PodState::Created | PodState::Removed | PodState::Killed => {
                    Operation::Abort(Some(log_error_status!(
                        "stop-container-bad-state",
                        &name.component
                    )(pod.state)))
                }
            },
            None => Operation::Abort(Some(log_error_status!(
                "stop-container-not-found",
                &name.component
            )(()))),
        }) {
            Compute::Updated {
                old: _,
                new: (_, pod),
            } => {
                log_info!("stop-container-success", &name.component, name.pod);
                if prior_state == PodState::Running {
                    // If the pod was previously `Running`, then we have to kill it.
                    if let Some(killer) = pod.killer.take() {
                        Ok(Some(killer))
                    } else {
                        // This situation should be logically impossible.
                        Err(log_error_status!(
                            "stop-container-no-killer",
                            &name.component
                        )(name.pod))
                    }
                } else {
                    // Otherwise, there's nothing to kill
                    // (if it was previously `Starting`, then `start_container` has to kill it).
                    Ok(None)
                }
            }
            Compute::Aborted(None) => Ok(None),
            Compute::Aborted(Some(error)) => Err(error),
            _ => {
                // Logically impossible (all possible compute outcomes are handled).
                Err(Status::internal("stop-container-impossible"))
            }
        }
    }

    pub(crate) fn remove_container(&self, name: &PodName) -> Result<()> {
        let pods = self.pods.pin();
        match pods.compute(name.pod, |entry| match entry {
            Some((_, pod)) => match pod.state {
                PodState::Stopped => {
                    // If the pod was previously `Running`, then we have to stop it.
                    // If it was previously `Starting`, then `start_container` has to stop it.
                    // Otherwise, killing the pod is as simple as updating the state.
                    let mut pod = pod.clone();
                    pod.state = PodState::Removed;
                    Operation::Insert(pod)
                }
                PodState::Removed => {
                    log_info!("remove-container-idempotent", &name.component, ());
                    Operation::Abort(None)
                }
                PodState::Initiated
                | PodState::Created
                | PodState::Starting
                | PodState::Running
                | PodState::Killed => Operation::Abort(Some(log_error_status!(
                    "remove-container-bad-state",
                    &name.component
                )(pod.state))),
            },
            None => Operation::Abort(Some(log_error_status!(
                "remove-container-not-found",
                &name.component
            )(()))),
        }) {
            Compute::Updated {
                old: _,
                new: (_, _),
            } => {
                log_info!("remove-container-success", &name.component, name.pod);
                Ok(())
            }
            Compute::Aborted(None) => Ok(()),
            Compute::Aborted(Some(error)) => Err(error),
            _ => {
                // Logically impossible (all possible compute outcomes are handled).
                Err(Status::internal("remove-container-impossible"))
            }
        }
    }

    /// Stop a running container / pod by killing the running server (if necessary)
    /// and transitioning the pod to [`Stopped`](PodState::Stopped).
    /// Attempts graceful server shutdown at first,
    /// waiting at most `timeout` before forcefully aborting.
    /// If `free_address` is `true`, also frees the pod's IP address.
    pub(crate) async fn kill_pod(&self, name: &PodName) -> Result<()> {
        if let Some((killer, ip_address)) = self.kill_pod_without_wait(name)? {
            // If the pod must be killed, do that before freeing the IP address.
            if let Some(killer) = killer.take() {
                // Give it a courtesy second to shut down gracefully.
                if !killer.kill_with_timeout(Duration::from_secs(1)).await {
                    log_warn!("kill-pod-stopped-forcefully", &name.component, name.pod);
                }
            }
            let _ = ip_address.deactivate().await?;
            let _ = ip_address.deallocate().await?;
        }
        Ok(())
    }

    /// See [`kill_pod`](Self::kill_pod).
    ///
    /// Similar to [`start_container_without_wait`](Self::start_container_without_wait),
    /// This function only exists to implement the state change synchronously.
    fn kill_pod_without_wait(
        &self,
        name: &PodName,
    ) -> Result<Option<(SingleUse<ContainerKiller>, IpAddress)>> {
        let mut prior_state = PodState::Removed;
        let pods = self.pods.pin();
        match pods.compute(name.pod, |entry| match entry {
            Some((_, pod)) => match pod.state {
                PodState::Initiated
                | PodState::Created
                | PodState::Starting
                | PodState::Running
                | PodState::Stopped
                | PodState::Removed => {
                    // If the pod was previously `Running`, then we have to stop it.
                    // If it was previously `Starting`, then `start_container` has to stop it.
                    // Otherwise, killing the pod is as simple as updating the state.
                    let mut pod = pod.clone();
                    prior_state = pod.state;
                    pod.state = PodState::Killed;
                    Operation::Insert(pod)
                }
                PodState::Killed => {
                    log_info!("kill-pod-idempotent", &name.component, name.pod);
                    Operation::Abort(None)
                }
            },
            None => Operation::Abort(Some(log_error_status!(
                "kill-pod-not-found",
                &name.component
            )(()))),
        }) {
            Compute::Updated {
                old: _,
                new: (_, pod),
            } => {
                log_info!("kill-pod-success", &name.component, name.pod);
                Ok(Some((pod.killer.clone(), pod.ip_address.clone())))
            }
            Compute::Aborted(None) => Ok(None),
            Compute::Aborted(Some(error)) => Err(error),
            _ => {
                // Logically impossible (all possible compute outcomes are handled).
                Err(Status::internal("kill-pod-impossible"))
            }
        }
    }

    pub(crate) fn delete_pod(&self, name: &PodName) -> Result<()> {
        let pods = self.pods.pin();
        match pods.compute(name.pod, |entry| match entry {
            Some((_, pod)) => match pod.state {
                PodState::Initiated
                | PodState::Created
                | PodState::Starting
                | PodState::Running
                | PodState::Stopped
                | PodState::Removed => {
                    // The CRI API promises
                    // that `StopPodSandbox` is called before `RemovePodSandbox`,
                    // so this should be impossible.
                    Operation::Abort(log_error_status!("delete-pod-bad-state", &name.component)(
                        pod.state,
                    ))
                }
                PodState::Killed => Operation::Remove,
            },
            None => Operation::Abort(Status::internal("delete-pod-not-found")),
        }) {
            Compute::Removed(_, _) => {
                log_info!("delete-pod-success", &name.component, name.pod);
                Ok(())
            }
            Compute::Aborted(error) => Err(error),
            _ => {
                // Logically impossible (all possible compute outcomes are handled).
                Err(Status::internal("delete-pod-impossible"))
            }
        }
    }

    /// Like [`Self::list_pods`],
    /// but with the added `name` condition for exact match by ID.
    /// Skips the exhaustive search and adds at most 1 pod to results.
    /// Does nothing if the pod can't be found.
    pub(crate) fn get_pod<T, F>(
        &self,
        name: &PodName,
        labels: &Vec<(&String, &String)>,
        states: &[PodState],
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        if let Some(pod) = self.pods.pin().get(&name.pod) {
            Self::match_pod(
                name.pod,
                pod,
                &pod.pod_labels,
                labels,
                states,
                transform,
                results,
            );
        }
    }

    /// Like [`Self::list_containers`],
    /// but with the added `name` condition for exact match by ID.
    /// Skips the exhaustive search and adds at most 1 container to results.
    /// Does nothing if the container can't be found.
    pub(crate) fn get_container<T, F>(
        &self,
        name: &PodName,
        labels: &Vec<(&String, &String)>,
        states: &[PodState],
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        if let Some(pod) = self.pods.pin().get(&name.pod) {
            Self::match_pod(
                name.pod,
                pod,
                &pod.container_labels,
                labels,
                states,
                transform,
                results,
            );
        }
    }

    /// List all the pods that match the given labels and states.
    /// See [`match_pod`](Self::match_pod) for matching details.
    /// Push results into the provided vector.
    ///
    /// Currently implemented by searching the pod map exhaustively (*O(n)*).
    /// YAGNIndices?
    pub(crate) fn list_pods<T, F>(
        &self,
        labels: &Vec<(&String, &String)>,
        states: &[PodState],
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        for (id, pod) in self.pods.pin().iter() {
            Self::match_pod(
                *id,
                pod,
                &pod.pod_labels,
                labels,
                states,
                transform,
                results,
            );
        }
    }

    /// List all the containers that match the given labels and states.
    /// See [`match_pod`](Self::match_pod) for matching details.
    /// Push results into the provided vector.
    ///
    /// Currently implemented by searching the pod map exhaustively (*O(n)*).
    /// YAGNIndices?
    pub(crate) fn list_containers<T, F>(
        &self,
        labels: &Vec<(&String, &String)>,
        states: &[PodState],
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        for (id, pod) in self.pods.pin().iter() {
            Self::match_pod(
                *id,
                pod,
                &pod.container_labels,
                labels,
                states,
                transform,
                results,
            );
        }
    }

    /// Logic common to [`get_pod`](Self::get_pod), [`get_container`](Self::get_container),
    /// [`list_pods`](Self::list_pods), and [`list_containers`](Self::list_containers).
    ///
    /// Check if the given pod (represented by `pod_id`, `pod`, and `labels`)
    /// matches the given filter conditions (`expected_labels` and `expected_states`).
    /// States matche either if the slice is empty or it includes the pod's state.
    /// Labels matches if every expected label is found in `labels`.
    ///
    /// If the pod matches, append it to `results`,
    /// after passing it through `transform`.
    #[inline(always)]
    fn match_pod<T, F>(
        pod_id: PodId,
        pod: &Pod,
        labels: &HashMap<String, String>,
        expected_labels: &Vec<(&String, &String)>,
        expected_states: &[PodState],
        transform: &F,
        results: &mut Vec<T>,
    ) where
        F: Fn(&PodName, &Pod) -> T,
    {
        // As a special case, an empty `expected_states` slice matches all states.
        if (expected_states.is_empty() || expected_states.contains(&pod.state))
            && expected_labels.iter().all(|(key, value)| {
                // Look up each key, which must be present, and check that the value matches.
                labels.get(*key).map_or(false, |actual| actual == *value)
            })
        {
            // This clone is not strictly necessary
            // We just need something `Display` that looks like a `PodName`.
            let name = PodName::new(pod.component_name.as_ref().clone(), pod_id);
            results.push(transform(&name, pod));
        }
    }
}

/// If `left` contains any entries
/// where the key starts with [`VIMANA_LABEL_PREFIX`]
/// and the entry does not exist with the same value in `right`,
/// log that difference as an error with `error_tag` and return [`Err`].
/// Otherwise, return [`Ok`].
fn check_vimana_labels(
    left: &HashMap<String, String>,
    right: &HashMap<String, String>,
    error_tag: &'static str,
    name: &PodName,
) -> Option<Status> {
    let unmatched: Vec<(&String, &String)> = left
        .iter()
        .filter(|(key, value)| {
            key.starts_with(VIMANA_LABEL_PREFIX) && right.get(*key) != Some(value)
        })
        .collect();
    if unmatched.is_empty() {
        None
    } else {
        Some(log_error_status!(error_tag, &name.component)(unmatched))
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

/// Used to shut down a running container. Can only be used once.
struct ContainerKiller {
    /// Send to this channel to shut down the server gracefully.
    shutdown: oneshot::Sender<()>,

    /// Useful for two things:
    /// - Awaiting graceful shutdown after sending the signal to [`shutdown`](Self::shutdown).
    /// - Forcibly shutting down.
    join: JoinHandle<StdResult<(), ServerError>>,
}

impl ContainerKiller {
    /// Attempt to kill the container gracefully at first.
    /// If that fails, or the timeout expires while waiting for graceful shut down to complete,
    /// forcefully abort the task instead.
    ///
    /// Return `true` if the container shut down gracefully
    /// and `false` if it was forcefully aborted.
    async fn kill_with_timeout(self, duration: Duration) -> bool {
        let aborter = self.join.abort_handle();
        if self.shutdown.send(()).is_ok() && timeout(duration, self.join).await.is_ok() {
            true
        } else {
            aborter.abort();
            false
        }
    }

    /// Kill a container immediately. In-flight requests are simply dropped.
    fn forcefully_abort(self) {
        self.join.abort();
    }
}

/// A cloneable handle to a singleton object that can be used at most once.
///
/// Can either be [empty](Self::default) or [populated](Self::of).
/// When populated, the inner value can be [taken](Self::take) making the `SingleUse` empty.
/// When empty, `take` returns an error.
struct SingleUse<T>(Arc<SyncMutex<Option<T>>>);

impl<T> SingleUse<T> {
    /// Return a populated handle with the given value.
    fn of(value: T) -> Self {
        Self(Arc::new(SyncMutex::new(Some(value))))
    }

    /// If populated, mutate `self` to become [empty](Self::default) and return the inner value.
    /// If `self` is already empty, return `None`.
    fn take(&self) -> Option<T> {
        match self.0.lock() {
            Ok(mut guard) => guard.take(),
            // Would indicate that some other thread panicked while holding the lock,
            // which should be logically impossible.
            Err(_poisoned) => None,
        }
    }
}

impl<T> Default for SingleUse<T> {
    /// Return an empty handle.
    /// Attempting to [take](Self::take) it will result in an error.
    fn default() -> Self {
        Self(Arc::new(SyncMutex::new(None)))
    }
}

impl<T> Clone for SingleUse<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// Return non-leap nanoseconds since 1970-01-01 00:00:00 UTC+0 as `i64`.
// Return zero if executed before 1970. Wraps around in 2262.
pub(crate) fn now() -> i64 {
    (SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64
        % (i64::MAX as u64)) as i64
}
