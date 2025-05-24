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
/// Version of the Vimana container runtime.
pub(crate) const CONTAINER_RUNTIME_VERSION: &str = "0.0.0";
/// Version of the CRI API supported by the runtime.
const CONTAINER_RUNTIME_API_VERSION: &str = "v1";

/// Prefix used to differentiate OCI pods / containers.
const OCI_PREFIX: &str = "O:";
/// Prefix used to differentiate Vimana pods.
const POD_PREFIX: &str = "P:";
/// Prefix used to differentiate Vimana containers.
const CONTAINER_PREFIX: &str = "C:";

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
    oci_runtime: AsyncMutex<RuntimeServiceClient<Channel>>,
}

/// If the given ID (mutable `String`) starts with the OCI prefix,
/// mutate it in-place to remove the prefix
/// and return early with the result of the given block.
/// Otherwise, assume it starts with a workd prefix and continue
/// (without mutating the ID).
macro_rules! intercept_oci_prefix {
    ( $id:expr, $downstream:block) => {
        let id_value = $id;
        if id_value.starts_with(OCI_PREFIX) {
            $id = String::from(&id_value[OCI_PREFIX.len()..]);
            return $downstream;
        }
        // If it doesn't start with the OCI prefix,
        // it must start with one of the workd prefixes.
        debug_assert!(id_value.starts_with(POD_PREFIX) || id_value.starts_with(CONTAINER_PREFIX));
        $id = id_value;
    };
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
fn oci_prefix<S: Display>(id: S) -> String {
    format!("{OCI_PREFIX}{id}")
}

#[inline(always)]
fn pod_prefix<S: Display>(id: S) -> String {
    format!("{POD_PREFIX}{id}")
}

#[inline(always)]
fn container_prefix<S: Display>(id: S) -> String {
    format!("{CONTAINER_PREFIX}{id}")
}

/// Inserts the OCI prefix in front of the string that lives at the end of the ID path.
///
/// E.g. `insert_oci_prefix!(x, foo, bar, baz)` expands to:
///
///     if let Some(ref mut foo) = &mut x.foo {
///         if let Some(ref mut bar) = &mut foo.bar {
///             bar.baz = oci_prefix(&bar.baz);
///         }
///     }
macro_rules! insert_oci_prefix {
    ( $r:ident, $id:ident ) => {
        $r.$id = oci_prefix(&$r.$id);
    };
    ( $r:ident, $id:ident, $( $i:ident ),+ ) => {
        if let Some(ref mut $id) = &mut $r.$id {
            insert_oci_prefix!($id, $($i),*);
        }
    };
}

/// Expands to a lambda function
/// that can be used to map the result of a downstream proxy call.
/// Re-inserts the OCI prefix to the single ID with the given name in the response.
macro_rules! map_oci_prefix {
    ( $( $id:ident ),+ ) => {
        |mut response| {
            let r = response.get_mut();
            insert_oci_prefix!(r, $($id),*);
            response
        }
    };
}

#[async_trait]
impl RuntimeService for ProxyingRuntimeService {
    async fn version(&self, _r: Request<v1::VersionRequest>) -> TonicResult<v1::VersionResponse> {
        Ok(Response::new(v1::VersionResponse {
            version: String::from(KUBELET_API_VERSION),
            runtime_name: String::from(CONTAINER_RUNTIME_NAME),
            runtime_version: String::from(CONTAINER_RUNTIME_VERSION),
            runtime_api_version: String::from(CONTAINER_RUNTIME_API_VERSION),
        }))
    }

    async fn run_pod_sandbox(
        &self,
        r: Request<v1::RunPodSandboxRequest>,
    ) -> TonicResult<v1::RunPodSandboxResponse> {
        let request = r.into_inner();

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if request.runtime_handler != CONTAINER_RUNTIME_NAME {
            return self
                .oci_runtime
                .lock()
                .await
                .run_pod_sandbox(Request::new(request))
                .await
                .map(map_oci_prefix!(pod_sandbox_id));
        }

        let config = request.config.unwrap_or_default();
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
        r: Request<v1::StopPodSandboxRequest>,
    ) -> TonicResult<v1::StopPodSandboxResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.pod_sandbox_id, {
            self.oci_runtime
                .lock()
                .await
                .stop_pod_sandbox(Request::new(request))
                .await
        });

        let name = parse_pod_prefixed_name(&request.pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        self.runtime.kill_pod(&name).await.log_error(&name)?;

        Ok(Response::new(v1::StopPodSandboxResponse {}))
    }

    async fn remove_pod_sandbox(
        &self,
        r: Request<v1::RemovePodSandboxRequest>,
    ) -> TonicResult<v1::RemovePodSandboxResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.pod_sandbox_id, {
            self.oci_runtime
                .lock()
                .await
                .remove_pod_sandbox(Request::new(request))
                .await
        });

        let name = parse_pod_prefixed_name(&request.pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        self.runtime.delete_pod(&name).log_error(&name)?;

        Ok(Response::new(v1::RemovePodSandboxResponse {}))
    }

    async fn pod_sandbox_status(
        &self,
        r: Request<v1::PodSandboxStatusRequest>,
    ) -> TonicResult<v1::PodSandboxStatusResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.pod_sandbox_id, {
            self.oci_runtime
                .lock()
                .await
                .pod_sandbox_status(Request::new(request))
                .await
                .map(|mut result| {
                    let response = result.get_mut();
                    insert_oci_prefix!(response, status, id);
                    for container_status in response.containers_statuses.iter_mut() {
                        insert_oci_prefix!(container_status, id);
                    }
                    result
                })
        });

        let name = parse_pod_prefixed_name(&request.pod_sandbox_id)
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
            || Err(anyhow!("Pod sandbox not found: {:?}", name)).log_error(&name),
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
        r: Request<v1::ListPodSandboxRequest>,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        let mut request = r.into_inner();

        // If there's a filter ID for a given runtime,
        // we can eliminate like half the work.
        if let Some(ref mut filter) = &mut request.filter {
            if filter.id.starts_with(OCI_PREFIX) {
                filter.id = String::from(&filter.id[OCI_PREFIX.len()..]);
                return self
                    .list_pod_sandbox_downstream(Request::new(request))
                    .await;
            } else if filter.id.starts_with(POD_PREFIX) {
                filter.id = String::from(&filter.id[POD_PREFIX.len()..]);
                return self.list_pod_sandbox_upstream(request);
            }
        }

        // Otherwise, combine the results of both runtimes
        // to get a complete picture of all pod sandboxes.
        self.list_pod_sandbox_downstream(Request::new(request.clone()))
            .await
            .and_then(|mut downstream_result| {
                // Upstream is the `workd` runtime.
                self.list_pod_sandbox_upstream(request)
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
        r: Request<v1::CreateContainerRequest>,
    ) -> TonicResult<v1::CreateContainerResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.pod_sandbox_id, {
            self.oci_runtime
                .lock()
                .await
                .create_container(Request::new(request))
                .await
                .map(map_oci_prefix!(container_id))
        });

        let name = parse_pod_prefixed_name(&request.pod_sandbox_id)
            .context("Invalid pod sandbox ID")
            .log_error(GlobalLogs)?;

        let config = request.config.unwrap_or_default();
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
        //if image_spec.runtime_handler != CONTAINER_RUNTIME_NAME {
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
                &image_spec,
            )
            .log_error(&name)?;

        Ok(Response::new(v1::CreateContainerResponse {
            container_id: container_prefix(name),
        }))
    }

    async fn start_container(
        &self,
        r: Request<v1::StartContainerRequest>,
    ) -> TonicResult<v1::StartContainerResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .start_container(Request::new(request))
                .await
        });

        let name = parse_container_prefixed_name(&request.container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        self.runtime.start_container(&name).await.log_error(&name)?;

        Ok(Response::new(v1::StartContainerResponse {}))
    }

    async fn stop_container(
        &self,
        r: Request<v1::StopContainerRequest>,
    ) -> TonicResult<v1::StopContainerResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .stop_container(Request::new(request))
                .await
        });

        let name = parse_container_prefixed_name(&request.container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        let timeout = Duration::from_secs(request.timeout.try_into().unwrap_or(0));
        self.runtime
            .stop_container(&name, timeout)
            .await
            .log_error(&name)?;

        Ok(Response::new(v1::StopContainerResponse {}))
    }

    async fn remove_container(
        &self,
        r: Request<v1::RemoveContainerRequest>,
    ) -> TonicResult<v1::RemoveContainerResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .remove_container(Request::new(request))
                .await
        });

        let name = parse_container_prefixed_name(&request.container_id)
            .context("Invalid container ID")
            .log_error(GlobalLogs)?;

        self.runtime.remove_container(&name).log_error(&name)?;

        Ok(Response::new(v1::RemoveContainerResponse {}))
    }

    async fn list_containers(
        &self,
        r: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        let mut request = r.into_inner();

        // If there's a filter ID with a runtime prefix,
        // use it to eliminate like half the work.
        if let Some(ref mut filter) = &mut request.filter {
            if filter.id.starts_with(OCI_PREFIX) {
                filter.id = String::from(&filter.id[OCI_PREFIX.len()..]);
                return self.list_containers_downstream(Request::new(request)).await;
            } else if filter.id.starts_with(CONTAINER_PREFIX) {
                filter.id = String::from(&filter.id[CONTAINER_PREFIX.len()..]);
                return self.list_containers_upstream(request);
            }
        }

        // Otherwise, combine the results with the downstream runtime.
        let r = self
            .list_containers_downstream(Request::new(request.clone()))
            .await
            .and_then(|mut downstream_result| {
                self.list_containers_upstream(request)
                    .map(|upstream_result| {
                        downstream_result
                            .get_mut()
                            .containers
                            .append(&mut upstream_result.into_inner().containers);
                        downstream_result
                    })
            });
        return r;
    }

    async fn container_status(
        &self,
        r: Request<v1::ContainerStatusRequest>,
    ) -> TonicResult<v1::ContainerStatusResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .container_status(Request::new(request))
                .await
                .map(map_oci_prefix!(status, id))
        });

        let name = parse_container_prefixed_name(&request.container_id)
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
            || Err(anyhow!("Container not found: {:?}", name)).log_error(&name),
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
        r: Request<v1::UpdateContainerResourcesRequest>,
    ) -> TonicResult<v1::UpdateContainerResourcesResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .update_container_resources(Request::new(request))
                .await
        });

        todo!()
    }

    async fn reopen_container_log(
        &self,
        r: Request<v1::ReopenContainerLogRequest>,
    ) -> TonicResult<v1::ReopenContainerLogResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .reopen_container_log(Request::new(request))
                .await
        });

        todo!()
    }

    async fn exec_sync(
        &self,
        r: Request<v1::ExecSyncRequest>,
    ) -> TonicResult<v1::ExecSyncResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .exec_sync(Request::new(request))
                .await
        });

        todo!()
    }

    async fn exec(&self, r: Request<v1::ExecRequest>) -> TonicResult<v1::ExecResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .exec(Request::new(request))
                .await
        });

        todo!()
    }

    async fn attach(&self, r: Request<v1::AttachRequest>) -> TonicResult<v1::AttachResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .attach(Request::new(request))
                .await
        });

        todo!()
    }

    async fn port_forward(
        &self,
        r: Request<v1::PortForwardRequest>,
    ) -> TonicResult<v1::PortForwardResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.pod_sandbox_id, {
            self.oci_runtime
                .lock()
                .await
                .port_forward(Request::new(request))
                .await
        });

        todo!()
    }

    async fn container_stats(
        &self,
        r: Request<v1::ContainerStatsRequest>,
    ) -> TonicResult<v1::ContainerStatsResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .container_stats(Request::new(request))
                .await
                .map(map_oci_prefix!(stats, attributes, id))
        });

        todo!()
    }

    async fn list_container_stats(
        &self,
        r: Request<v1::ListContainerStatsRequest>,
    ) -> TonicResult<v1::ListContainerStatsResponse> {
        let request = r.into_inner();

        // TODO: Figure out how to list container stats upstream as well.
        self.oci_runtime
            .lock()
            .await
            .list_container_stats(Request::new(request))
            .await
    }

    async fn pod_sandbox_stats(
        &self,
        r: Request<v1::PodSandboxStatsRequest>,
    ) -> TonicResult<v1::PodSandboxStatsResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.pod_sandbox_id, {
            self.oci_runtime
                .lock()
                .await
                .pod_sandbox_stats(Request::new(request))
                .await
                .map(map_oci_prefix!(stats, attributes, id))
        });

        todo!()
    }

    async fn list_pod_sandbox_stats(
        &self,
        r: Request<v1::ListPodSandboxStatsRequest>,
    ) -> TonicResult<v1::ListPodSandboxStatsResponse> {
        let request = r.into_inner();

        // TODO: Figure out how to list pod stats upstream as well.
        self.oci_runtime
            .lock()
            .await
            .list_pod_sandbox_stats(Request::new(request))
            .await
    }

    async fn update_runtime_config(
        &self,
        r: Request<v1::UpdateRuntimeConfigRequest>,
    ) -> TonicResult<v1::UpdateRuntimeConfigResponse> {
        let request = r.into_inner();

        // TODO: Figure out how to update config upstream as well.
        self.oci_runtime
            .lock()
            .await
            .update_runtime_config(Request::new(request))
            .await
    }

    async fn status(&self, r: Request<v1::StatusRequest>) -> TonicResult<v1::StatusResponse> {
        let request = r.into_inner();

        // These are the only 2 required conditions.
        let mut runtime_ready_condition = v1::RuntimeCondition {
            r#type: String::from(CONDITION_RUNTIME_READY),
            status: true,
            reason: String::default(),
            message: String::default(),
        };
        let mut network_ready_condition = v1::RuntimeCondition {
            r#type: String::from(CONDITION_NETWORK_READY),
            status: true,
            reason: String::default(),
            message: String::default(),
        };

        // TODO: Populate these with non-placeholder information.
        let mut info = HashMap::default();
        let mut runtime_handlers = Vec::default();

        match self
            .oci_runtime
            .lock()
            .await
            .status(Request::new(request.clone()))
            .await
        {
            Ok(downstream_response) => {
                let downstream_response = downstream_response.into_inner();
                // TODO: Adjust upstream conditions based on downstream conditions.
                info.extend(downstream_response.info);
                runtime_handlers.extend(downstream_response.runtime_handlers);
            }
            Err(downstream_error) => {
                // The downstream runtime must function.
                return Err(downstream_error);
            }
        }

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
        r: Request<v1::CheckpointContainerRequest>,
    ) -> TonicResult<v1::CheckpointContainerResponse> {
        let mut request = r.into_inner();

        intercept_oci_prefix!(request.container_id, {
            self.oci_runtime
                .lock()
                .await
                .checkpoint_container(Request::new(request))
                .await
        });

        todo!()
    }

    type GetContainerEventsStream = ReceiverStream<StdResult<v1::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        r: Request<v1::GetEventsRequest>,
    ) -> TonicResult<Self::GetContainerEventsStream> {
        let request = r.into_inner();

        // TODO: Figure out how streaming works.
        return Err(Status::internal("GetContainerEvents TODO"));
    }

    async fn list_metric_descriptors(
        &self,
        r: Request<v1::ListMetricDescriptorsRequest>,
    ) -> TonicResult<v1::ListMetricDescriptorsResponse> {
        let request = r.into_inner();

        // TODO: Also merge in stats about the upstream system!
        self.oci_runtime
            .lock()
            .await
            .list_metric_descriptors(Request::new(request))
            .await
    }

    async fn list_pod_sandbox_metrics(
        &self,
        r: Request<v1::ListPodSandboxMetricsRequest>,
    ) -> TonicResult<v1::ListPodSandboxMetricsResponse> {
        let request = r.into_inner();

        // TODO: Also merge in stats about the upstream system!
        self.oci_runtime
            .lock()
            .await
            .list_pod_sandbox_metrics(Request::new(request))
            .await
    }

    async fn runtime_config(
        &self,
        r: Request<v1::RuntimeConfigRequest>,
    ) -> TonicResult<v1::RuntimeConfigResponse> {
        let request = r.into_inner();

        // TODO: Also merge in stats about the upstream system!
        self.oci_runtime
            .lock()
            .await
            .runtime_config(Request::new(request))
            .await
    }
}

impl ProxyingRuntimeService {
    pub(crate) fn new(runtime: WorkRuntime, oci_runtime: RuntimeServiceClient<Channel>) -> Self {
        Self {
            runtime,
            oci_runtime: AsyncMutex::new(oci_runtime),
        }
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
            if let Ok(name) = Name::parse(&filter.id).pod() {
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
            if let Ok(name) = Name::parse(&filter.id).pod() {
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

    /// Invoke the downstream OCI runtime with the given request as-is.
    /// Intercept and edit the response to prefix pod sandbox IDs.
    async fn list_pod_sandbox_downstream(
        &self,
        r: Request<v1::ListPodSandboxRequest>,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        let result = self.oci_runtime.lock().await.list_pod_sandbox(r).await;

        return result.map(|mut response| {
            let r = response.get_mut();
            for item in r.items.iter_mut() {
                item.id = oci_prefix(&item.id);
            }
            response
        });
    }

    /// Invoke the downstream OCI runtime with the given request as-is.
    /// Intercept and edit the response to prefix pod sandbox and container IDs.
    async fn list_containers_downstream(
        &self,
        r: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        let result = self.oci_runtime.lock().await.list_containers(r).await;

        return result.map(|mut response| {
            let r = response.get_mut();
            for container in r.containers.iter_mut() {
                container.id = oci_prefix(&container.id);
                container.pod_sandbox_id = oci_prefix(&container.pod_sandbox_id);
            }
            response
        });
    }
}

/// Convert the internal pod to a CRI-API [v1::PodSandbox] to return in `ListPodSandbox`.
fn cri_pod_sandbox(name: &PodName, pod: &Pod) -> v1::PodSandbox {
    v1::PodSandbox {
        id: pod_prefix(name),
        // All Workd containers use the same runtime.
        runtime_handler: String::from(CONTAINER_RUNTIME_NAME),
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
            runtime_handler: String::from(CONTAINER_RUNTIME_NAME),
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
