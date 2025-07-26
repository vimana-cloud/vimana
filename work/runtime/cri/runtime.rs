//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! `RuntimeService` for the Work Node runtime.
//!
//! K8s control plane pods expect an OCI-compatible runtime.
//! Since Vimana's Wasm component runtime is not OCI-compatible,
//! this implementation relies on a downstream OCI runtime to run control plane pods,
//! enabling colocation of pods using diverse runtimes on a single node.
//!
//! Business logic does not belong in this file.
//! Its purpose is to accept incoming CRI API requests from clients,
//! and either proxy them to the downstream runtime
//! and/or access the Wasm component map.
//! It also transparently inserts and removes prefixes
//! to each container and pod sandbox ID in responses and requests, respectively,
//! to distinguish which runtime each belongs to.

use std::collections::HashMap;
use std::fmt::Display;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use api_proto::runtime::v1;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use api_proto::runtime::v1::runtime_service_server::RuntimeService;
use papaya::HashSet as LockFreeConcurrentHashSet;
use tokio::sync::Mutex as AsyncMutex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::channel::Channel;
use tonic::{async_trait, Request, Response, Status};

use crate::cri::{component_name_from_labels, GlobalLogs, LogErrorToStatus, TonicResult};
use crate::state::{now, Pod, PodState};
use crate::WorkRuntime;
use names::{Name, PodName};

/// "For now it expects 0.1.0." - https://github.com/cri-o/cri-o/blob/v1.31.3/server/version.go.
const KUBELET_API_VERSION: &str = "0.1.0";
/// Name of the Vimana container runtime.
pub(crate) const CONTAINER_RUNTIME_NAME: &str = "workd";
/// Name of the Vimana container runtime handler.
pub(super) const CONTAINER_RUNTIME_HANDLER: &str = "workd-handler";
/// Version of the Vimana container runtime.
pub(crate) const CONTAINER_RUNTIME_VERSION: &str = "0.0.0";
/// Version of the CRI API supported by the runtime.
const CONTAINER_RUNTIME_API_VERSION: &str = "v1";

/// Prefix used to differentiate Vimana pods.
const POD_PREFIX: &str = "p-";
/// Prefix used to differentiate Vimana containers.
const CONTAINER_PREFIX: &str = "c-";

/// All pod states for which a container "exists".
const POD_STATES_CONTAINER_ALL: [PodState; 4] = [
    PodState::Created,
    PodState::Starting,
    PodState::Running,
    PodState::Stopped,
];
/// Pod states matching [`v1::ContainerState::ContainerCreated`].
const POD_STATES_CONTAINER_CREATED: [PodState; 2] = [PodState::Created, PodState::Starting];
/// Pod states matching [`v1::ContainerState::ContainerRunning`].
const POD_STATES_CONTAINER_RUNNING: [PodState; 1] = [PodState::Running];
/// Pod states matching [`v1::ContainerState::ContainerExited`].
const POD_STATES_CONTAINER_EXITED: [PodState; 3] =
    [PodState::Stopped, PodState::Removed, PodState::Killed];
/// Pod states matching [`v1::ContainerState::ContainerUnknown`].
const POD_STATES_CONTAINER_UNKNOWN: [PodState; 0] = [];

// Required conditions for [`v1::StatusResponse`]:

const CONDITION_RUNTIME_READY: &str = "RuntimeReady";
const CONDITION_NETWORK_READY: &str = "NetworkReady";

/// Wrapper around [WorkRuntime] that implements [RuntimeService]
/// with a downstream server for OCI requests.
pub(crate) struct ProxyingRuntimeService {
    /// The upstream runtime handler for all Vimana-related business logic.
    runtime: WorkRuntime,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    downstream: AsyncMutex<RuntimeServiceClient<Channel>>,

    // TODO: Report the size of this data structure in some sort of runtime stats.
    /// The set of all pod sandbox IDs and container IDs managed by the downstream runtime.
    /// In `containerd`, pod sandbox IDs are just the container ID for the pause container,
    /// so lumping those two seemingly distinct namespaces together makes a degree of sense.
    downstream_ids: LockFreeConcurrentHashSet<String>,
}

#[inline(always)]
fn parse_pod_prefixed_name(name: &str) -> Result<PodName> {
    debug_assert!(name.starts_with(POD_PREFIX));
    Name::parse(&name[POD_PREFIX.len()..]).pod()
}

#[inline(always)]
fn parse_container_prefixed_name(name: &str) -> Result<PodName> {
    debug_assert!(name.starts_with(CONTAINER_PREFIX));
    Name::parse(&name[CONTAINER_PREFIX.len()..]).pod()
}

#[inline(always)]
fn pod_prefix<S: Display>(id: S) -> String {
    format!("{POD_PREFIX}{id}")
}

#[inline(always)]
fn container_prefix<S: Display>(id: S) -> String {
    format!("{CONTAINER_PREFIX}{id}")
}

#[async_trait]
impl RuntimeService for ProxyingRuntimeService {
    async fn version(
        &self,
        _request: Request<v1::VersionRequest>,
    ) -> TonicResult<v1::VersionResponse> {
        Ok(Response::new(v1::VersionResponse {
            version: String::from(KUBELET_API_VERSION),
            runtime_name: String::from(CONTAINER_RUNTIME_NAME),
            runtime_version: String::from(CONTAINER_RUNTIME_VERSION),
            runtime_api_version: String::from(CONTAINER_RUNTIME_API_VERSION),
        }))
    }

    async fn run_pod_sandbox(
        &self,
        request: Request<v1::RunPodSandboxRequest>,
    ) -> TonicResult<v1::RunPodSandboxResponse> {
        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if request.get_ref().runtime_handler != CONTAINER_RUNTIME_HANDLER {
            let response = self.downstream.lock().await.run_pod_sandbox(request).await;
            if let Ok(reply) = &response {
                let pod_sandbox_id = reply.get_ref().pod_sandbox_id.clone();
                self.downstream_ids.pin().insert(pod_sandbox_id);
            }
            return response;
        }

        let config = request.into_inner().config.unwrap_or_default();
        let component_name = component_name_from_labels(&config.labels)
            .with_context(|| format!("Invalid pod labels: {:?}", config.labels))
            .log_error(GlobalLogs)?;

        // Check that the request fits into Vimana's narrow vision of validity
        // for the sake of preventing unexpected behavior.
        if !config.port_mappings.is_empty() {
            // gRPC pods are never expected to have a port mapping.
            return Err(anyhow!("gRPC port mappings are unsupported")).log_error(&component_name);
        }

        let component_name = Arc::new(component_name);
        let pod_name = self
            .runtime
            .init_pod(
                component_name.clone(),
                config.metadata.unwrap_or_default(),
                config.labels,
                config.annotations,
            )
            .await
            .log_error(component_name.as_ref())?;

        Ok(Response::new(v1::RunPodSandboxResponse {
            // Prefix the ID so we can distinguish it from downstream OCI pod IDs.
            pod_sandbox_id: pod_prefix(&pod_name),
        }))
    }

    async fn stop_pod_sandbox(
        &self,
        request: Request<v1::StopPodSandboxRequest>,
    ) -> TonicResult<v1::StopPodSandboxResponse> {
        if self.is_downstream(&request.get_ref().pod_sandbox_id) {
            return self.downstream.lock().await.stop_pod_sandbox(request).await;
        }

        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        self.runtime.kill_pod(&name).await.log_error(&name)?;

        Ok(Response::new(v1::StopPodSandboxResponse {}))
    }

    async fn remove_pod_sandbox(
        &self,
        request: Request<v1::RemovePodSandboxRequest>,
    ) -> TonicResult<v1::RemovePodSandboxResponse> {
        if self.is_downstream(&request.get_ref().pod_sandbox_id) {
            let pod_sandbox_id = request.get_ref().pod_sandbox_id.clone();
            let response = self
                .downstream
                .lock()
                .await
                .remove_pod_sandbox(request)
                .await;
            if response.is_ok() {
                self.downstream_ids.pin().remove(&pod_sandbox_id);
            }
            return response;
        }

        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        self.runtime.delete_pod(&name).log_error(&name)?;

        Ok(Response::new(v1::RemovePodSandboxResponse {}))
    }

    async fn pod_sandbox_status(
        &self,
        request: Request<v1::PodSandboxStatusRequest>,
    ) -> TonicResult<v1::PodSandboxStatusResponse> {
        if self.is_downstream(&request.get_ref().pod_sandbox_id) {
            return self
                .downstream
                .lock()
                .await
                .pod_sandbox_status(request)
                .await;
        }

        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        let mut pod_sandbox_status = Vec::with_capacity(1);
        self.runtime.get_pod(
            &name,
            &Vec::default(),
            None,
            &cri_pod_sandbox_status,
            &mut pod_sandbox_status,
        );
        let timestamp = now();

        pod_sandbox_status.pop().map_or_else(
            || Err(Status::not_found(name.to_string())),
            |(pod_status, container_statuses)| {
                Ok(Response::new(v1::PodSandboxStatusResponse {
                    status: Some(pod_status),
                    info: HashMap::default(),
                    containers_statuses: container_statuses,
                    timestamp,
                }))
            },
        )
    }

    async fn list_pod_sandbox(
        &self,
        request: Request<v1::ListPodSandboxRequest>,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        // Combine the results of both runtimes to get a complete picture of all pod sandboxes.
        // In theory, there might be a filter on pod sandbox ID
        // that would obviate the need to search both runtimes,
        // but in practice kubelet never populates the ID field in the filter.
        self.downstream
            .lock()
            .await
            .list_pod_sandbox(Request::new(request.get_ref().clone()))
            .await
            .and_then(|mut downstream_result| {
                // Upstream is the `workd` runtime.
                self.list_pod_sandbox_upstream(request.into_inner())
                    .map(|upstream_result| {
                        downstream_result
                            .get_mut()
                            .items
                            .append(&mut upstream_result.into_inner().items);
                        downstream_result
                    })
            })
    }

    async fn create_container(
        &self,
        request: Request<v1::CreateContainerRequest>,
    ) -> TonicResult<v1::CreateContainerResponse> {
        if self.is_downstream(&request.get_ref().pod_sandbox_id) {
            let response = self.downstream.lock().await.create_container(request).await;
            if let Ok(reply) = &response {
                self.downstream_ids
                    .pin()
                    .insert(reply.get_ref().container_id.clone());
            }
            return response;
        }

        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        let config = request.into_inner().config.unwrap_or_default();
        //let component = component_name_from_labels(&config.labels)?;

        // While redundant, the component name from the container's labels
        // must match the component name extracted from the pod ID and image ID.
        //if component != name.component {
        //    return Err(Status::invalid_argument(
        //        "create-container-labels-pod-mismatch",
        //    ));
        //}

        // Check that the image spec also matches the labels / pod name.
        // In fact, the whole `ImageSpec` is essentially determined by the component name.
        let image_spec = config.image.unwrap_or_default();
        //if image_spec.image != name.component.to_string() {
        //    return Err(Status::invalid_argument(
        //        "create-container-labels-image-mismatch",
        //    ));
        //}
        // YAGNI: multiple handlers
        //if image_spec.runtime_handler != CONTAINER_RUNTIME_HANDLER {
        //    return Err(Status::invalid_argument("create-container-invalid-runtime"));
        //}
        // No particular reason there can't be annotations or a user specified image;
        // just keeping a minimum API surface while we figure things out.
        if !image_spec.annotations.is_empty() {
            return Err(anyhow!("Image spec annotations are unsupported")).log_error(&name);
        }
        //if !image_spec.user_specified_image.is_empty() {
        //    return Err(Status::invalid_argument(
        //        "create-container-user-specified-image",
        //    ));
        //}

        let mut environment = HashMap::with_capacity(config.envs.len());
        for key_value in config.envs.iter() {
            environment.insert(key_value.key.clone(), key_value.value.clone());
        }

        // The CRI API has separate steps for creating pods and creating containers,
        // but a component pod is inseparable from its single container,
        // so "pods" and containers are created simultaneously.
        self.runtime
            .create_container(
                &name,
                &config.metadata,
                &config.labels,
                &config.annotations,
                &environment,
                &Some(image_spec),
            )
            .log_error(&name)?;

        Ok(Response::new(v1::CreateContainerResponse {
            container_id: container_prefix(name),
        }))
    }

    async fn start_container(
        &self,
        request: Request<v1::StartContainerRequest>,
    ) -> TonicResult<v1::StartContainerResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.start_container(request).await;
        }

        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        self.runtime.start_container(&name).await.log_error(&name)?;

        Ok(Response::new(v1::StartContainerResponse {}))
    }

    async fn stop_container(
        &self,
        request: Request<v1::StopContainerRequest>,
    ) -> TonicResult<v1::StopContainerResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.stop_container(request).await;
        }

        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;
        let timeout = Duration::from_secs(request.get_ref().timeout.try_into().unwrap_or(0));

        self.runtime
            .stop_container(&name, timeout)
            .await
            .log_error(&name)?;

        Ok(Response::new(v1::StopContainerResponse {}))
    }

    async fn remove_container(
        &self,
        request: Request<v1::RemoveContainerRequest>,
    ) -> TonicResult<v1::RemoveContainerResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            let container_id = request.get_ref().container_id.clone();
            let response = self.downstream.lock().await.remove_container(request).await;
            if response.is_ok() {
                self.downstream_ids.pin().remove(&container_id);
            }
            return response;
        }

        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        self.runtime.remove_container(&name).log_error(&name)?;

        Ok(Response::new(v1::RemoveContainerResponse {}))
    }

    async fn list_containers(
        &self,
        request: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        // Combine the results of both runtimes to get a complete picture of all containers.
        // In theory, there might be a filter on container ID
        // that would obviate the need to search both runtimes,
        // but in practice kubelet never populates the ID field in the filter.
        self.downstream
            .lock()
            .await
            .list_containers(Request::new(request.get_ref().clone()))
            .await
            .and_then(|mut downstream_result| {
                self.list_containers_upstream(request.into_inner())
                    .map(|upstream_result| {
                        downstream_result
                            .get_mut()
                            .containers
                            .append(&mut upstream_result.into_inner().containers);
                        downstream_result
                    })
            })
    }

    async fn container_status(
        &self,
        request: Request<v1::ContainerStatusRequest>,
    ) -> TonicResult<v1::ContainerStatusResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.container_status(request).await;
        }

        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        let mut container_status = Vec::with_capacity(1);
        self.runtime.get_container(
            &name,
            &Vec::default(),
            &POD_STATES_CONTAINER_ALL,
            &cri_container_status,
            &mut container_status,
        );

        container_status.pop().map_or_else(
            || Err(Status::not_found(name.to_string())),
            |status| {
                Ok(Response::new(v1::ContainerStatusResponse {
                    status: Some(status),
                    info: HashMap::default(),
                }))
            },
        )
    }

    async fn update_container_resources(
        &self,
        request: Request<v1::UpdateContainerResourcesRequest>,
    ) -> TonicResult<v1::UpdateContainerResourcesResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self
                .downstream
                .lock()
                .await
                .update_container_resources(request)
                .await;
        }

        todo!()
    }

    async fn reopen_container_log(
        &self,
        request: Request<v1::ReopenContainerLogRequest>,
    ) -> TonicResult<v1::ReopenContainerLogResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self
                .downstream
                .lock()
                .await
                .reopen_container_log(request)
                .await;
        }

        todo!()
    }

    async fn exec_sync(
        &self,
        request: Request<v1::ExecSyncRequest>,
    ) -> TonicResult<v1::ExecSyncResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.exec_sync(request).await;
        }

        todo!()
    }

    async fn exec(&self, request: Request<v1::ExecRequest>) -> TonicResult<v1::ExecResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.exec(request).await;
        }

        todo!()
    }

    async fn attach(&self, request: Request<v1::AttachRequest>) -> TonicResult<v1::AttachResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.attach(request).await;
        }

        todo!()
    }

    async fn port_forward(
        &self,
        request: Request<v1::PortForwardRequest>,
    ) -> TonicResult<v1::PortForwardResponse> {
        if self.is_downstream(&request.get_ref().pod_sandbox_id) {
            return self.downstream.lock().await.port_forward(request).await;
        }

        todo!()
    }

    async fn container_stats(
        &self,
        request: Request<v1::ContainerStatsRequest>,
    ) -> TonicResult<v1::ContainerStatsResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self.downstream.lock().await.container_stats(request).await;
        }

        todo!()
    }

    async fn list_container_stats(
        &self,
        request: Request<v1::ListContainerStatsRequest>,
    ) -> TonicResult<v1::ListContainerStatsResponse> {
        // TODO: Figure out how to list container stats upstream as well.
        self.downstream
            .lock()
            .await
            .list_container_stats(request)
            .await
    }

    async fn pod_sandbox_stats(
        &self,
        request: Request<v1::PodSandboxStatsRequest>,
    ) -> TonicResult<v1::PodSandboxStatsResponse> {
        if self.is_downstream(&request.get_ref().pod_sandbox_id) {
            return self
                .downstream
                .lock()
                .await
                .pod_sandbox_stats(request)
                .await;
        }

        todo!()
    }

    async fn list_pod_sandbox_stats(
        &self,
        request: Request<v1::ListPodSandboxStatsRequest>,
    ) -> TonicResult<v1::ListPodSandboxStatsResponse> {
        // TODO: Figure out how to list pod stats upstream as well.
        self.downstream
            .lock()
            .await
            .list_pod_sandbox_stats(request)
            .await
    }

    async fn update_runtime_config(
        &self,
        request: Request<v1::UpdateRuntimeConfigRequest>,
    ) -> TonicResult<v1::UpdateRuntimeConfigResponse> {
        // TODO: Figure out how to update config upstream as well.
        self.downstream
            .lock()
            .await
            .update_runtime_config(request)
            .await
    }

    async fn status(&self, request: Request<v1::StatusRequest>) -> TonicResult<v1::StatusResponse> {
        //// These are the only 2 required conditions.
        //let mut runtime_ready_condition = v1::RuntimeCondition {
        //    r#type: String::from(CONDITION_RUNTIME_READY),
        //    status: true,
        //    reason: String::default(),
        //    message: String::default(),
        //};
        //let mut network_ready_condition = v1::RuntimeCondition {
        //    r#type: String::from(CONDITION_NETWORK_READY),
        //    status: true,
        //    reason: String::default(),
        //    message: String::default(),
        //};

        //// TODO: Populate these with relevant information.
        //let mut info = HashMap::default();
        //let mut runtime_handlers = Vec::default();

        match self
            .downstream
            .lock()
            .await
            .status(Request::new(request.get_ref().clone()))
            .await
        {
            Ok(downstream_response) => {
                return Ok(downstream_response);
                //let downstream_response = downstream_response.into_inner();
                //// TODO: Adjust upstream conditions based on downstream conditions.
                //info.extend(downstream_response.info);
                //runtime_handlers.extend(downstream_response.runtime_handlers);
            }
            Err(downstream_error) => {
                // TODO: Don't fail closed on the downstream runtime if it's not necessary.
                return Err(downstream_error);
            }
        }

        //Ok(Response::new(v1::StatusResponse {
        //    status: Some(v1::RuntimeStatus {
        //        conditions: vec![runtime_ready_condition, network_ready_condition],
        //    }),
        //    info,
        //    runtime_handlers,
        //    features: None,
        //}))
    }

    async fn checkpoint_container(
        &self,
        request: Request<v1::CheckpointContainerRequest>,
    ) -> TonicResult<v1::CheckpointContainerResponse> {
        if self.is_downstream(&request.get_ref().container_id) {
            return self
                .downstream
                .lock()
                .await
                .checkpoint_container(request)
                .await;
        }

        todo!()
    }

    type GetContainerEventsStream = ReceiverStream<StdResult<v1::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        request: Request<v1::GetEventsRequest>,
    ) -> TonicResult<Self::GetContainerEventsStream> {
        // TODO: Figure out how streaming works.
        return Err(Status::internal("GetContainerEvents TODO"));
    }

    async fn list_metric_descriptors(
        &self,
        request: Request<v1::ListMetricDescriptorsRequest>,
    ) -> TonicResult<v1::ListMetricDescriptorsResponse> {
        // TODO: Also merge in stats about the upstream system!
        self.downstream
            .lock()
            .await
            .list_metric_descriptors(request)
            .await
    }

    async fn list_pod_sandbox_metrics(
        &self,
        request: Request<v1::ListPodSandboxMetricsRequest>,
    ) -> TonicResult<v1::ListPodSandboxMetricsResponse> {
        // TODO: Also merge in stats about the upstream system!
        self.downstream
            .lock()
            .await
            .list_pod_sandbox_metrics(request)
            .await
    }

    async fn runtime_config(
        &self,
        request: Request<v1::RuntimeConfigRequest>,
    ) -> TonicResult<v1::RuntimeConfigResponse> {
        // TODO: Also merge in stats about the upstream system!
        self.downstream.lock().await.runtime_config(request).await
    }

    async fn update_pod_sandbox_resources(
        &self,
        r: Request<v1::UpdatePodSandboxResourcesRequest>,
    ) -> TonicResult<v1::UpdatePodSandboxResourcesResponse> {
        todo!()
    }
}

impl ProxyingRuntimeService {
    pub(crate) async fn new(
        runtime: WorkRuntime,
        mut downstream: RuntimeServiceClient<Channel>,
    ) -> Result<Self> {
        // On startup, list any pre-existing pod sandboxes or containers in the downstream runtime,
        // so requests that reference them can be routed appropriately.
        let downstream_ids = LockFreeConcurrentHashSet::new();
        {
            let downstream_pods = downstream
                .list_pod_sandbox(Request::new(v1::ListPodSandboxRequest::default()))
                .await
                .context("Failed to list existing pod sandboxes from the downstream runtime")?;
            let downstream_ids = downstream_ids.pin();
            for pod in &downstream_pods.get_ref().items {
                downstream_ids.insert(pod.id.clone());
            }
        }
        {
            let downstream_containers = downstream
                .list_containers(Request::new(v1::ListContainersRequest::default()))
                .await
                .context("Failed to list existing containers from the downstream runtime")?;
            let downstream_ids = downstream_ids.pin();
            for container in &downstream_containers.get_ref().containers {
                downstream_ids.insert(container.id.clone());
            }
        }

        Ok(Self {
            runtime,
            downstream: AsyncMutex::new(downstream),
            downstream_ids,
        })
    }

    /// Return true iff a pod or container ID should be managed by the downstream runtime.
    fn is_downstream(&self, id: &str) -> bool {
        // If the ID does *not* start with a Vimana prefix, then it must be downstream.
        // However, just because it does start with the Vimana prefix
        // does not necessarily mean it does *not* belong downstream.
        self.downstream_ids.pin().contains(id)
            || !(id.starts_with(POD_PREFIX) || id.starts_with(CONTAINER_PREFIX))
    }

    /// Perform sandbox listing in the workd runtime.
    fn list_pod_sandbox_upstream(
        &self,
        request: v1::ListPodSandboxRequest,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        let mut response = v1::ListPodSandboxResponse::default();

        // Every condition in the filter is composed with AND.
        // The default filter if none is provided has no conditions (always passes).
        let filter = request.filter.unwrap_or_default();
        // Collect the required labels as a vector for easier iteration.
        let labels: Vec<(&String, &String)> = filter.label_selector.iter().collect();
        let readiness = filter
            .state
            .map(|state| state.state == v1::PodSandboxState::SandboxReady as i32);

        // Filter ID, if present, can speed things up a lot.
        if filter.id.len() > 0 {
            // I believe the ID must match exactly,
            // but that's not entirely clear from the documentation,
            // which just says "ID of the sandbox".
            if let Ok(name) = parse_pod_prefixed_name(&filter.id) {
                // If it's a complete, parseable pod name (after the prefix),
                // look it up and return it, if the other conditions are met.
                self.runtime.get_pod(
                    &name,
                    &labels,
                    readiness,
                    &cri_pod_sandbox,
                    &mut response.items,
                );
            }
            // Otherwise, the whole filter fails to match anything,
            // because all conditions are required and the ID condition is impossible.
        } else {
            // If the ID filter is absent,
            // search exhaustively based on the state and labels filters.
            self.runtime
                .list_pods(&labels, readiness, &cri_pod_sandbox, &mut response.items);
        }

        Ok(Response::new(response))
    }

    /// Perform sandbox listing in the workd runtime.
    fn list_containers_upstream(
        &self,
        request: v1::ListContainersRequest,
    ) -> TonicResult<v1::ListContainersResponse> {
        let mut response = v1::ListContainersResponse::default();

        // Every condition in the filter is composed with AND.
        // The default filter if none is provided has no conditions (always passes).
        let filter = request.filter.unwrap_or_default();
        // Collect the required labels as a vector for easier iteration.
        let labels: Vec<(&String, &String)> = filter.label_selector.iter().collect();
        let matching_states: &[PodState] = filter
            .state
            .map_or(&POD_STATES_CONTAINER_ALL, cri_container_state_to_pod_states);

        // Filter ID, if present, can speed things up a lot.
        if filter.id.len() > 0 {
            // I believe the ID must match exactly,
            // but that's not entirely clear from the documentation,
            // which just says "ID of the container".
            if let Ok(name) = parse_container_prefixed_name(&filter.id) {
                // If it's a complete, parseable pod name (after the prefix),
                // look it up and return it, if the other conditions are met.
                self.runtime.get_container(
                    &name,
                    &labels,
                    matching_states,
                    &cri_container,
                    &mut response.containers,
                );
            }
            // Otherwise, the whole filter fails to match anything,
            // because all conditions are required and the ID condition is impossible.
        } else {
            // If the ID filter is absent,
            // search exhaustively based on the state and labels filters.
            self.runtime.list_containers(
                &labels,
                matching_states,
                &cri_container,
                &mut response.containers,
            );
        }

        Ok(Response::new(response))
    }
}

/// Convert the internal pod to a CRI-API [v1::PodSandbox] to return in `ListPodSandbox`.
fn cri_pod_sandbox(name: &PodName, pod: &Pod) -> v1::PodSandbox {
    v1::PodSandbox {
        id: pod_prefix(name),
        // All Workd containers use the same runtime.
        runtime_handler: String::from(CONTAINER_RUNTIME_HANDLER),
        // Pod sandboxes are always ready (containers might not be).
        state: pod_state_to_cri_pod_state(pod.state) as i32,
        // The rest are just cloned from the controller:
        metadata: Some(pod.pod_sandbox_metadata.clone()),
        created_at: pod.pod_created_at,
        labels: pod.pod_labels.clone(),
        annotations: pod.pod_annotations.clone(),
    }
}

/// Convert the internal pod to a CRI-API [v1::Container] to return in `ListContainers`.
fn cri_container(name: &PodName, pod: &Pod) -> v1::Container {
    v1::Container {
        id: container_prefix(name),
        pod_sandbox_id: pod_prefix(name),
        metadata: pod.container_metadata.clone(),
        image: pod.image_spec.clone(),
        image_ref: cri_image_ref(),
        state: pod_state_to_cri_container_state(pod.state) as i32,
        created_at: pod.container_created_at,
        labels: pod.container_labels.clone(),
        annotations: pod.container_annotations.clone(),
        image_id: cri_image_id(),
    }
}

/// Convert the internal pod to a CRI-API [v1::PodSandboxStatus] to return in `PodSandboxStatus`.
/// Also return the container status, if there is one
/// (as either an empty vector or a singleton vector).
fn cri_pod_sandbox_status(
    name: &PodName,
    pod: &Pod,
) -> (v1::PodSandboxStatus, Vec<v1::ContainerStatus>) {
    (
        v1::PodSandboxStatus {
            id: pod_prefix(name),
            metadata: Some(pod.pod_sandbox_metadata.clone()),
            state: pod_state_to_cri_pod_state(pod.state) as i32,
            created_at: pod.pod_created_at,
            network: Some(v1::PodSandboxNetworkStatus {
                ip: pod.ip_address.to_string(),
                additional_ips: Vec::default(),
            }),
            linux: None,
            labels: pod.pod_labels.clone(),
            annotations: pod.pod_annotations.clone(),
            runtime_handler: String::from(CONTAINER_RUNTIME_HANDLER),
        },
        match pod.state {
            PodState::Initiated | PodState::Removed | PodState::Killed => Vec::default(),
            PodState::Created | PodState::Starting | PodState::Running | PodState::Stopped => {
                vec![cri_container_status(name, pod)]
            }
        },
    )
}

/// Convert the internal pod to a CRI-API [v1::ContainerStatus] to return in `ContainerStatus`.
fn cri_container_status(name: &PodName, pod: &Pod) -> v1::ContainerStatus {
    v1::ContainerStatus {
        id: container_prefix(name),
        metadata: pod.container_metadata.clone(),
        state: pod_state_to_cri_container_state(pod.state) as i32,
        created_at: pod.container_created_at,
        started_at: pod.container_started_at,
        finished_at: pod.container_finished_at,
        exit_code: 0, // TODO: Populate this in case a container fails at runtime.
        image: pod.image_spec.clone(),
        image_ref: cri_image_ref(),
        reason: String::from("TODO"),
        message: String::from("TODO"),
        labels: pod.container_labels.clone(),
        annotations: pod.container_annotations.clone(),
        // Vimana containers never have volume mounts.
        mounts: Vec::default(),
        log_path: cri_container_log_path(),
        // TODO: Resource limiting information.
        resources: None,
        image_id: cri_image_id(),
        // Wasm modules do not use user-based privileges.
        user: None,
        stop_signal: v1::Signal::Sigterm as i32,
    }
}

fn pod_state_to_cri_pod_state(state: PodState) -> v1::PodSandboxState {
    match state {
        PodState::Initiated
        | PodState::Created
        | PodState::Starting
        | PodState::Running
        | PodState::Stopped
        | PodState::Removed => v1::PodSandboxState::SandboxReady,
        PodState::Killed => v1::PodSandboxState::SandboxNotready,
    }
}

const CONTAINER_STATE_CREATED_VALUE: i32 = v1::ContainerState::ContainerCreated as i32;
const CONTAINER_STATE_RUNNING_VALUE: i32 = v1::ContainerState::ContainerRunning as i32;
const CONTAINER_STATE_EXITED_VALUE: i32 = v1::ContainerState::ContainerExited as i32;
const CONTAINER_STATE_UNKNOWN_VALUE: i32 = v1::ContainerState::ContainerUnknown as i32;

fn cri_container_state_to_pod_states(state: v1::ContainerStateValue) -> &'static [PodState] {
    match state.state {
        CONTAINER_STATE_CREATED_VALUE => &POD_STATES_CONTAINER_CREATED,
        CONTAINER_STATE_RUNNING_VALUE => &POD_STATES_CONTAINER_RUNNING,
        CONTAINER_STATE_EXITED_VALUE => &POD_STATES_CONTAINER_EXITED,
        CONTAINER_STATE_UNKNOWN_VALUE => &POD_STATES_CONTAINER_UNKNOWN,
        // This fallback should be impossible
        // (unless the set of possible enum values expands).
        _ => &POD_STATES_CONTAINER_ALL,
    }
}

fn pod_state_to_cri_container_state(state: PodState) -> v1::ContainerState {
    match state {
        PodState::Initiated | PodState::Removed | PodState::Killed => {
            v1::ContainerState::ContainerUnknown
        }
        PodState::Created | PodState::Starting => v1::ContainerState::ContainerCreated,
        PodState::Running => v1::ContainerState::ContainerRunning,
        PodState::Stopped => v1::ContainerState::ContainerExited,
    }
}

fn cri_image_ref() -> String {
    String::from("TODO")
}

fn cri_image_id() -> String {
    String::from("TODO")
}

fn cri_container_log_path() -> String {
    // Logging happens entirely via OTLP, not files.
    String::from("/dev/null")
}
