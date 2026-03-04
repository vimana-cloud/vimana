//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! `RuntimeService` for the Work Node runtime.
//!
//! Business logic does not belong in this file.
//! Its purpose is to accept incoming CRI API requests from clients
//! and access the Wasm component map.

use std::collections::HashMap;
use std::fmt::Display;
use std::net::IpAddr;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cri_api_proto::runtime::v1;
use cri_api_proto::runtime::v1::runtime_service_server::RuntimeService;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, async_trait};

use crate::WorkRuntime;
use crate::cri::{GlobalLogs, LogErrorToStatus, TonicResult, component_name_from_labels};
use crate::state::{Pod, PodState, now};
use logging::log_info_globally;
use names::{Name, PodName};

/// "For now it expects 0.1.0." - https://github.com/cri-o/cri-o/blob/v1.31.3/server/version.go.
const KUBELET_API_VERSION: &str = "0.1.0";
/// Name of the Vimana container runtime.
pub(crate) const CONTAINER_RUNTIME_NAME: &str = "vimana";
/// Name of the Vimana container runtime handler.
pub(super) const CONTAINER_RUNTIME_HANDLER: &str = "vimana-handler";
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

#[allow(dead_code)]
const CONDITION_RUNTIME_READY: &str = "RuntimeReady";
#[allow(dead_code)]
const CONDITION_NETWORK_READY: &str = "NetworkReady";

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
impl RuntimeService for WorkRuntime {
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
        // Fail unless the `vimanad` runtime is explicitly specified.
        let handler = &request.get_ref().runtime_handler;
        if handler != CONTAINER_RUNTIME_HANDLER {
            return Err(
                if let Some(config) = &request.get_ref().config
                    && let Some(metadata) = &config.metadata
                {
                    anyhow!(
                        "Unknown runtime handler {:?} for pod {:?}",
                        handler,
                        &metadata.name
                    )
                } else {
                    anyhow!("Unknown runtime handler {:?}", handler)
                },
            )
            .log_error(GlobalLogs);
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
            .init_pod(
                component_name.clone(),
                config.metadata.unwrap_or_default(),
                config.labels,
                config.annotations,
            )
            .await
            .log_error(component_name.as_ref())?;

        Ok(Response::new(v1::RunPodSandboxResponse {
            // Prefix the ID so kubelet can distinguish it from its associated container.
            pod_sandbox_id: pod_prefix(&pod_name),
        }))
    }

    async fn stop_pod_sandbox(
        &self,
        request: Request<v1::StopPodSandboxRequest>,
    ) -> TonicResult<v1::StopPodSandboxResponse> {
        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        self.kill_pod(&name).await.log_error(&name)?;

        Ok(Response::new(v1::StopPodSandboxResponse {}))
    }

    async fn remove_pod_sandbox(
        &self,
        request: Request<v1::RemovePodSandboxRequest>,
    ) -> TonicResult<v1::RemovePodSandboxResponse> {
        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        self.delete_pod(&name).log_error(&name)?;

        Ok(Response::new(v1::RemovePodSandboxResponse {}))
    }

    async fn pod_sandbox_status(
        &self,
        request: Request<v1::PodSandboxStatusRequest>,
    ) -> TonicResult<v1::PodSandboxStatusResponse> {
        let name = parse_pod_prefixed_name(&request.get_ref().pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        let mut pod_sandbox_status = Vec::with_capacity(1);
        self.get_pod(
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
        let mut response = v1::ListPodSandboxResponse::default();

        // Every condition in the filter is composed with AND.
        // The default filter if none is provided has no conditions (always passes).
        let filter = request.into_inner().filter.unwrap_or_default();
        // Collect the required labels as a vector for easier iteration.
        let labels: Vec<(&String, &String)> = filter.label_selector.iter().collect();
        let readiness = filter
            .state
            .map(|state| state.state == v1::PodSandboxState::SandboxReady as i32);

        // Filter ID, if present, can speed things up a lot.
        if !filter.id.is_empty() {
            // I believe the ID must match exactly,
            // but that's not entirely clear from the documentation,
            // which just says "ID of the sandbox".
            if let Ok(name) = parse_pod_prefixed_name(&filter.id) {
                // If it's a complete, parseable pod name (after the prefix),
                // look it up and return it, if the other conditions are met.
                self.get_pod(
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
            self.list_pods(&labels, readiness, &cri_pod_sandbox, &mut response.items);
        }

        Ok(Response::new(response))
    }

    async fn create_container(
        &self,
        request: Request<v1::CreateContainerRequest>,
    ) -> TonicResult<v1::CreateContainerResponse> {
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
        self.create_container(
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
        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        self.start_container(&name).await.log_error(&name)?;

        Ok(Response::new(v1::StartContainerResponse {}))
    }

    async fn stop_container(
        &self,
        request: Request<v1::StopContainerRequest>,
    ) -> TonicResult<v1::StopContainerResponse> {
        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;
        let timeout = Duration::from_secs(request.get_ref().timeout.try_into().unwrap_or(0));

        self.stop_container(&name, timeout).await.log_error(&name)?;

        Ok(Response::new(v1::StopContainerResponse {}))
    }

    async fn remove_container(
        &self,
        request: Request<v1::RemoveContainerRequest>,
    ) -> TonicResult<v1::RemoveContainerResponse> {
        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        self.remove_container(&name).log_error(&name)?;

        Ok(Response::new(v1::RemoveContainerResponse {}))
    }

    async fn list_containers(
        &self,
        request: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        let mut response = v1::ListContainersResponse::default();

        // Every condition in the filter is composed with AND.
        // The default filter if none is provided has no conditions (always passes).
        let filter = request.into_inner().filter.unwrap_or_default();
        // Collect the required labels as a vector for easier iteration.
        let labels: Vec<(&String, &String)> = filter.label_selector.iter().collect();
        let matching_states: &[PodState] = filter
            .state
            .map_or(&POD_STATES_CONTAINER_ALL, cri_container_state_to_pod_states);

        // Filter ID, if present, can speed things up a lot.
        if !filter.id.is_empty() {
            // I believe the ID must match exactly,
            // but that's not entirely clear from the documentation,
            // which just says "ID of the container".
            if let Ok(name) = parse_container_prefixed_name(&filter.id) {
                // If it's a complete, parseable pod name (after the prefix),
                // look it up and return it, if the other conditions are met.
                self.get_container(
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
            self.list_containers(
                &labels,
                matching_states,
                &cri_container,
                &mut response.containers,
            );
        }

        Ok(Response::new(response))
    }

    async fn container_status(
        &self,
        request: Request<v1::ContainerStatusRequest>,
    ) -> TonicResult<v1::ContainerStatusResponse> {
        let name = parse_container_prefixed_name(&request.get_ref().container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        let mut container_status = Vec::with_capacity(1);
        self.get_container(
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
        _request: Request<v1::UpdateContainerResourcesRequest>,
    ) -> TonicResult<v1::UpdateContainerResourcesResponse> {
        todo!()
    }

    async fn reopen_container_log(
        &self,
        _request: Request<v1::ReopenContainerLogRequest>,
    ) -> TonicResult<v1::ReopenContainerLogResponse> {
        todo!()
    }

    async fn exec_sync(
        &self,
        _request: Request<v1::ExecSyncRequest>,
    ) -> TonicResult<v1::ExecSyncResponse> {
        todo!()
    }

    async fn exec(&self, _request: Request<v1::ExecRequest>) -> TonicResult<v1::ExecResponse> {
        todo!()
    }

    async fn attach(
        &self,
        _request: Request<v1::AttachRequest>,
    ) -> TonicResult<v1::AttachResponse> {
        todo!()
    }

    async fn port_forward(
        &self,
        _request: Request<v1::PortForwardRequest>,
    ) -> TonicResult<v1::PortForwardResponse> {
        todo!()
    }

    async fn container_stats(
        &self,
        _request: Request<v1::ContainerStatsRequest>,
    ) -> TonicResult<v1::ContainerStatsResponse> {
        todo!()
    }

    async fn list_container_stats(
        &self,
        _request: Request<v1::ListContainerStatsRequest>,
    ) -> TonicResult<v1::ListContainerStatsResponse> {
        todo!()
    }

    async fn pod_sandbox_stats(
        &self,
        _request: Request<v1::PodSandboxStatsRequest>,
    ) -> TonicResult<v1::PodSandboxStatsResponse> {
        todo!()
    }

    async fn list_pod_sandbox_stats(
        &self,
        _request: Request<v1::ListPodSandboxStatsRequest>,
    ) -> TonicResult<v1::ListPodSandboxStatsResponse> {
        todo!()
    }

    async fn update_runtime_config(
        &self,
        request: Request<v1::UpdateRuntimeConfigRequest>,
    ) -> TonicResult<v1::UpdateRuntimeConfigResponse> {
        if let Some(runtime_config) = &request.get_ref().runtime_config
            && let Some(network_config) = &runtime_config.network_config
            && !network_config.pod_cidr.is_empty()
        {
            validate_cidr(&network_config.pod_cidr)
                .with_context(|| format!("Invalid CIDR: {:?}", network_config.pod_cidr))
                .log_error(GlobalLogs)?;
            self.ipam.update_pod_cidr(&network_config.pod_cidr).await;
            log_info_globally!("Updated pod CIDR to {}", &network_config.pod_cidr);
        } else {
            // TODO: The docs say "If the CIDR is empty, runtimes should omit it"
            //   but it's unclear what exactly that means.
        }

        Ok(Response::new(v1::UpdateRuntimeConfigResponse {}))
    }

    async fn status(
        &self,
        _request: Request<v1::StatusRequest>,
    ) -> TonicResult<v1::StatusResponse> {
        // These are the only 2 required conditions.
        let runtime_ready_condition = v1::RuntimeCondition {
            r#type: String::from(CONDITION_RUNTIME_READY),
            status: true,
            reason: String::default(),
            message: String::default(),
        };
        let network_ready_condition = v1::RuntimeCondition {
            r#type: String::from(CONDITION_NETWORK_READY),
            status: true,
            reason: String::default(),
            message: String::default(),
        };

        let info = HashMap::default();
        let runtime_handlers = vec![v1::RuntimeHandler {
            name: String::from(CONTAINER_RUNTIME_HANDLER),
            features: None,
        }];

        Ok(Response::new(v1::StatusResponse {
            status: Some(v1::RuntimeStatus {
                conditions: vec![runtime_ready_condition, network_ready_condition],
            }),
            info,
            runtime_handlers,
            features: None,
        }))
    }

    async fn checkpoint_container(
        &self,
        _request: Request<v1::CheckpointContainerRequest>,
    ) -> TonicResult<v1::CheckpointContainerResponse> {
        todo!()
    }

    type GetContainerEventsStream = ReceiverStream<StdResult<v1::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        _request: Request<v1::GetEventsRequest>,
    ) -> TonicResult<Self::GetContainerEventsStream> {
        // TODO: Figure out how streaming works.
        return Err(Status::internal("GetContainerEvents TODO"));
    }

    async fn list_metric_descriptors(
        &self,
        _request: Request<v1::ListMetricDescriptorsRequest>,
    ) -> TonicResult<v1::ListMetricDescriptorsResponse> {
        todo!()
    }

    async fn list_pod_sandbox_metrics(
        &self,
        _request: Request<v1::ListPodSandboxMetricsRequest>,
    ) -> TonicResult<v1::ListPodSandboxMetricsResponse> {
        todo!()
    }

    async fn runtime_config(
        &self,
        _request: Request<v1::RuntimeConfigRequest>,
    ) -> TonicResult<v1::RuntimeConfigResponse> {
        Ok(Response::new(v1::RuntimeConfigResponse {
            linux: Some(v1::LinuxRuntimeConfiguration {
                // Vimana doesn't use cgroups,
                // but it does expect to run on a Debian-based system, which uses systemd.
                // Just tell Kubelet to use systemd so nobody steps on anybody else's toes.
                cgroup_driver: v1::CgroupDriver::Systemd as i32,
            }),
        }))
    }

    async fn update_pod_sandbox_resources(
        &self,
        _request: Request<v1::UpdatePodSandboxResourcesRequest>,
    ) -> TonicResult<v1::UpdatePodSandboxResourcesResponse> {
        todo!()
    }
}

/// Convert the internal pod to a CRI-API [v1::PodSandbox] to return in `ListPodSandbox`.
fn cri_pod_sandbox(name: &PodName, pod: &Pod) -> v1::PodSandbox {
    v1::PodSandbox {
        id: pod_prefix(name),
        // All Vimana containers use the same runtime.
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

/// Validate that a string is valid CIDR notation (e.g. `10.0.0.0/8` or `fc00::/48`).
fn validate_cidr(cidr: &str) -> Result<()> {
    let Some((ip_part, prefix_part)) = cidr.split_once('/') else {
        bail!("Missing slash");
    };
    let ip: IpAddr = ip_part.parse().context("Bad address")?;
    let prefix: u8 = prefix_part.parse().context("Bad prefix length")?;
    let max_prefix = if ip.is_ipv4() { 32 } else { 128 };
    if prefix > max_prefix {
        bail!("Prefix length {} exceeds maximum {}", prefix, max_prefix);
    }
    Ok(())
}
