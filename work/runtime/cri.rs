//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! for the Work Node runtime.
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
use std::fmt::{Debug, Display};
use std::sync::Arc;
use std::time::Duration;

use api_proto::runtime::v1;
use api_proto::runtime::v1::image_service_server::ImageService;
use api_proto::runtime::v1::runtime_service_server::RuntimeService;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{async_trait, Request, Response, Status};

use crate::state::{now, Pod, PodState};
use crate::WorkRuntime;
use error::Result;
use names::{ComponentName, DomainUuid, Name, PodName};

/// "For now it expects 0.1.0." - https://github.com/cri-o/cri-o/blob/v1.31.3/server/version.go.
const KUBELET_API_VERSION: &str = "0.1.0";
/// Name of the Vimana container runtime.
pub const CONTAINER_RUNTIME_NAME: &str = "workd";
/// Version of the Vimana container runtime.
pub const CONTAINER_RUNTIME_VERSION: &str = "0.0.0";
/// Version of the CRI API supported by the runtime.
const CONTAINER_RUNTIME_API_VERSION: &str = "v1";

/// Prefix used to differentiate OCI pods / containers.
const OCI_PREFIX: &str = "O:";
/// Prefix used to differentiate Vimana pods / containers.
const WORKD_PREFIX: &str = "W:";

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

// These labels must be present on every pod and container using the Vimana handler:

const LABEL_DOMAIN_KEY: &str = "vimana.host/domain";
const LABEL_SERVICE_KEY: &str = "vimana.host/service";
const LABEL_VERSION_KEY: &str = "vimana.host/version";

// Required conditions for [`v1::StatusResponse`]:

const CONDITION_RUNTIME_READY: &str = "RuntimeReady";
const CONDITION_NETWORK_READY: &str = "NetworkReady";

/// Wrapper around [WorkRuntime] that can implement [RuntimeService] and [ImageService]
/// without running afoul of Rust's rules on foreign types / traits.
pub struct VimanaCriService(pub Arc<WorkRuntime>);

/// Type boilerplate for a typical Tonic response result.
type TonicResult<T> = Result<Response<T>>;

/// Return early with the result of the given block
/// if the given ID (mutable `String`) starts with the OCI prefix.
/// Otherwise, assume it starts with the work prefix and continue.
/// Either way, update the ID in-place to remove the prefix.
macro_rules! intercept_prefix {
    ( $id:expr, $downstream:block) => {
        let id_value = $id;
        if id_value.starts_with(OCI_PREFIX) {
            $id = String::from(&id_value[OCI_PREFIX.len()..]);
            return $downstream;
        }
        // If it doesn't start with the OCI prefix, it must start with the workd prefix.
        debug_assert!(id_value.starts_with(WORKD_PREFIX));
        $id = id_value;
    };
}

#[inline(always)]
fn oci_prefix<S: Display>(id: S) -> String {
    format!("{OCI_PREFIX}{id}")
}

#[inline(always)]
fn workd_prefix<S: Display>(id: S) -> String {
    format!("{WORKD_PREFIX}{id}")
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
impl RuntimeService for VimanaCriService {
    async fn version(&self, r: Request<v1::VersionRequest>) -> TonicResult<v1::VersionResponse> {
        let request = r.into_inner();
        log_object("Version", &request);

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
        log_object("RunPodSandbox", &request);

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if request.runtime_handler != CONTAINER_RUNTIME_NAME {
            return self
                .0
                .oci_runtime
                .lock()
                .await
                .run_pod_sandbox(Request::new(request))
                .await
                .map(map_oci_prefix!(pod_sandbox_id));
        }

        let config = request.config.unwrap_or_default();
        let component = component_name_from_labels(&config.labels)?;

        // Check that the request fits into Vimana's narrow vision of validity
        // for the sake of preventing unexpected behavior.
        if !config.port_mappings.is_empty() {
            // gRPC pods are never expected to have a port mapping.
            return Err(Status::invalid_argument("grpc-port-mappings"));
        }

        let name = self
            .0
            .init_pod(
                component,
                config.metadata.unwrap_or_default(),
                config.labels,
                config.annotations,
            )
            .await?;

        Ok(Response::new(v1::RunPodSandboxResponse {
            // Prefix the ID so we can distinguish it from downstream OCI pod IDs.
            pod_sandbox_id: workd_prefix(&name),
        }))
    }

    async fn stop_pod_sandbox(
        &self,
        r: Request<v1::StopPodSandboxRequest>,
    ) -> TonicResult<v1::StopPodSandboxResponse> {
        let mut request = r.into_inner();
        log_object("StopPodSandbox", &request);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .stop_pod_sandbox(Request::new(request))
                .await
        });

        let name = Name::parse(&request.pod_sandbox_id[WORKD_PREFIX.len()..]).pod()?;
        self.0.kill_pod(&name).await?;

        Ok(Response::new(v1::StopPodSandboxResponse {}))
    }

    async fn remove_pod_sandbox(
        &self,
        r: Request<v1::RemovePodSandboxRequest>,
    ) -> TonicResult<v1::RemovePodSandboxResponse> {
        let mut request = r.into_inner();
        log_object("RemovePodSandbox", &request);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .remove_pod_sandbox(Request::new(request))
                .await
        });

        let name = Name::parse(&request.pod_sandbox_id[WORKD_PREFIX.len()..]).pod()?;
        self.0.delete_pod(&name)?;

        Ok(Response::new(v1::RemovePodSandboxResponse {}))
    }

    async fn pod_sandbox_status(
        &self,
        r: Request<v1::PodSandboxStatusRequest>,
    ) -> TonicResult<v1::PodSandboxStatusResponse> {
        let mut request = r.into_inner();
        log_object("PodSandboxStatus", &request);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
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

        if request.pod_sandbox_id.starts_with(WORKD_PREFIX) {
            let name = Name::parse(&request.pod_sandbox_id[WORKD_PREFIX.len()..]).pod()?;
            let mut pod_sandbox_status = Vec::with_capacity(1);
            self.0.get_pod(
                &name,
                &Vec::default(),
                None,
                &cri_pod_sandbox_status,
                &mut pod_sandbox_status,
            );
            let timestamp = now();
            pod_sandbox_status.pop().map_or(
                Err(Status::not_found("pod-sandbox-not-found")),
                |(pod_status, container_statuses)| {
                    Ok(Response::new(v1::PodSandboxStatusResponse {
                        status: Some(pod_status),
                        info: HashMap::default(),
                        containers_statuses: container_statuses,
                        timestamp,
                    }))
                },
            )
        } else {
            // The pod sandbox ID lacked either the `W:` prefix or the `O:` prefix.
            Err(Status::not_found("pod-sandbox-id-missing-prefix"))
        }
    }

    async fn list_pod_sandbox(
        &self,
        r: Request<v1::ListPodSandboxRequest>,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        let mut request = r.into_inner();
        log_object("ListPodSandbox", &request);

        // If there's a filter ID for a given runtime,
        // we can eliminate like half the work.
        if let Some(ref mut filter) = &mut request.filter {
            if filter.id.starts_with(OCI_PREFIX) {
                filter.id = String::from(&filter.id[OCI_PREFIX.len()..]);
                return self
                    .list_pod_sandbox_downstream(Request::new(request))
                    .await;
            } else if filter.id.starts_with(WORKD_PREFIX) {
                filter.id = String::from(&filter.id[WORKD_PREFIX.len()..]);
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
        log_object("CreateContainer", &request.pod_sandbox_id);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .create_container(Request::new(request))
                .await
                .map(map_oci_prefix!(container_id))
        });

        let name = Name::parse(&request.pod_sandbox_id[WORKD_PREFIX.len()..]).pod()?;
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
            return Err(Status::invalid_argument(
                "create-container-image-annotations",
            ));
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
        self.0.create_container(
            &name,
            &config.metadata,
            &config.labels,
            &config.annotations,
            &environment,
        )?;

        Ok(Response::new(v1::CreateContainerResponse {
            // Containers and their pod sandboxes share IDs.
            container_id: request.pod_sandbox_id,
        }))
    }

    async fn start_container(
        &self,
        r: Request<v1::StartContainerRequest>,
    ) -> TonicResult<v1::StartContainerResponse> {
        let mut request = r.into_inner();
        log_object("StartContainer", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .start_container(Request::new(request))
                .await
        });

        let name = Name::parse(&request.container_id[WORKD_PREFIX.len()..]).pod()?;
        self.0.start_container(&name).await?;

        Ok(Response::new(v1::StartContainerResponse {}))
    }

    async fn stop_container(
        &self,
        r: Request<v1::StopContainerRequest>,
    ) -> TonicResult<v1::StopContainerResponse> {
        let mut request = r.into_inner();
        log_object("StopContainer", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .stop_container(Request::new(request))
                .await
        });

        let name = Name::parse(&request.container_id[WORKD_PREFIX.len()..]).pod()?;
        let timeout = Duration::from_secs(request.timeout.try_into().unwrap_or(0));
        self.0.stop_container(&name, timeout).await?;

        Ok(Response::new(v1::StopContainerResponse {}))
    }

    async fn remove_container(
        &self,
        r: Request<v1::RemoveContainerRequest>,
    ) -> TonicResult<v1::RemoveContainerResponse> {
        let mut request = r.into_inner();
        log_object("RemoveContainer", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .remove_container(Request::new(request))
                .await
        });

        let name = Name::parse(&request.container_id[WORKD_PREFIX.len()..]).pod()?;
        self.0.remove_container(&name)?;

        Ok(Response::new(v1::RemoveContainerResponse {}))
    }

    async fn list_containers(
        &self,
        r: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        let mut request = r.into_inner();
        log_object("ListContainers", &request);

        // If there's a filter ID with a runtime prefix,
        // use it to eliminate like half the work.
        if let Some(ref mut filter) = &mut request.filter {
            if filter.id.starts_with(OCI_PREFIX) {
                filter.id = String::from(&filter.id[OCI_PREFIX.len()..]);
                return self.list_containers_downstream(Request::new(request)).await;
            } else if filter.id.starts_with(WORKD_PREFIX) {
                filter.id = String::from(&filter.id[WORKD_PREFIX.len()..]);
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
        log_object("ContainerStatus", &request.container_id);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .container_status(Request::new(request))
                .await
                .map(map_oci_prefix!(status, id))
        });

        if request.container_id.starts_with(WORKD_PREFIX) {
            let name = Name::parse(&request.container_id[WORKD_PREFIX.len()..]).pod()?;
            let mut container_status = Vec::with_capacity(1);
            self.0.get_container(
                &name,
                &Vec::default(),
                &POD_STATES_CONTAINER_ALL,
                &cri_container_status,
                &mut container_status,
            );
            container_status
                .pop()
                .map_or(Err(Status::not_found("container-not-found")), |status| {
                    Ok(Response::new(v1::ContainerStatusResponse {
                        status: Some(status),
                        info: HashMap::default(),
                    }))
                })
        } else {
            // The container ID lacked either the `W:` prefix or the `O:` prefix.
            Err(Status::not_found("container-id-missing-prefix"))
        }
    }

    async fn update_container_resources(
        &self,
        r: Request<v1::UpdateContainerResourcesRequest>,
    ) -> TonicResult<v1::UpdateContainerResourcesResponse> {
        let mut request = r.into_inner();
        log_object("UpdateContainerResources", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
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
        log_object("ReopenContainerLogRequest", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
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
        log_object("ExecSync", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .exec_sync(Request::new(request))
                .await
        });

        todo!()
    }

    async fn exec(&self, r: Request<v1::ExecRequest>) -> TonicResult<v1::ExecResponse> {
        let mut request = r.into_inner();
        log_object("Exec", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .exec(Request::new(request))
                .await
        });

        todo!()
    }

    async fn attach(&self, r: Request<v1::AttachRequest>) -> TonicResult<v1::AttachResponse> {
        let mut request = r.into_inner();
        log_object("Attach", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
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
        log_object("PortForward", &request);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
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
        log_object("ContainerStats", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
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
        log_object("ListContainerStats", &request);

        // TODO: Figure out how to list container stats upstream as well.
        self.0
            .oci_runtime
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
        log_object("PodSandboxStats", &request);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
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
        log_object("ListPodSandboxStats", &request);

        // TODO: Figure out how to list pod stats upstream as well.
        self.0
            .oci_runtime
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
        log_object("UpdateRuntimeConfig", &request);

        // TODO: Figure out how to update config upstream as well.
        self.0
            .oci_runtime
            .lock()
            .await
            .update_runtime_config(Request::new(request))
            .await
    }

    async fn status(&self, r: Request<v1::StatusRequest>) -> TonicResult<v1::StatusResponse> {
        let request = r.into_inner();
        log_object("Status", &request);

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
            .0
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
        log_object("CheckpointContainer", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .checkpoint_container(Request::new(request))
                .await
        });

        todo!()
    }

    type GetContainerEventsStream = ReceiverStream<Result<v1::ContainerEventResponse>>;

    async fn get_container_events(
        &self,
        r: Request<v1::GetEventsRequest>,
    ) -> TonicResult<Self::GetContainerEventsStream> {
        let request = r.into_inner();
        log_object("GetContainerEvents", &request);

        // TODO: Figure out how streaming works.
        return Err(Status::internal("GetContainerEvents TODO"));
    }

    async fn list_metric_descriptors(
        &self,
        r: Request<v1::ListMetricDescriptorsRequest>,
    ) -> TonicResult<v1::ListMetricDescriptorsResponse> {
        let request = r.into_inner();
        log_object("ListMetricDescriptors", &request);

        // TODO: Also merge in stats about the upstream system!
        self.0
            .oci_runtime
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
        log_object("ListPodSandboxMetrics", &request);

        // TODO: Also merge in stats about the upstream system!
        self.0
            .oci_runtime
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
        log_object("RuntimeConfig", &request);

        // TODO: Also merge in stats about the upstream system!
        self.0
            .oci_runtime
            .lock()
            .await
            .runtime_config(Request::new(request))
            .await
    }
}

#[async_trait]
impl ImageService for VimanaCriService {
    async fn list_images(
        &self,
        r: Request<v1::ListImagesRequest>,
    ) -> TonicResult<v1::ListImagesResponse> {
        let request = r.into_inner();
        log_object("ListImages", &request);

        let filter = request.clone().filter.unwrap_or_default();
        let spec = filter.image.unwrap_or_default();
        let handler = spec.runtime_handler;

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if handler != "TODO-this-should-be-something-else-but-what?" {
            return self
                .0
                .oci_image
                .lock()
                .await
                .list_images(Request::new(request))
                .await;
        }

        todo!()
    }

    async fn image_status(
        &self,
        r: Request<v1::ImageStatusRequest>,
    ) -> TonicResult<v1::ImageStatusResponse> {
        let request = r.into_inner();
        log_object("ImageStatus", &request);

        let spec = request.clone().image.unwrap_or_default();
        let handler = spec.runtime_handler;

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if handler != "TODO-this-should-be-something-else-but-what?" {
            return self
                .0
                .oci_image
                .lock()
                .await
                .image_status(Request::new(request))
                .await;
        }

        todo!()
    }

    async fn pull_image(
        &self,
        r: Request<v1::PullImageRequest>,
    ) -> TonicResult<v1::PullImageResponse> {
        let request = r.into_inner();
        log_object("PullImage", &request);

        let spec = request.clone().image.unwrap_or_default();
        let handler = spec.runtime_handler;

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if handler != "TODO-this-should-be-something-else-but-what?" {
            return self
                .0
                .oci_image
                .lock()
                .await
                .pull_image(Request::new(request))
                .await;
        }

        todo!()
    }

    async fn remove_image(
        &self,
        r: Request<v1::RemoveImageRequest>,
    ) -> TonicResult<v1::RemoveImageResponse> {
        let request = r.into_inner();
        log_object("RemoveImage", &request);

        let spec = request.clone().image.unwrap_or_default();
        let handler = spec.runtime_handler;

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if handler != "TODO-this-should-be-something-else-but-what?" {
            return self
                .0
                .oci_image
                .lock()
                .await
                .remove_image(Request::new(request))
                .await;
        }

        todo!()
    }

    async fn image_fs_info(
        &self,
        r: Request<v1::ImageFsInfoRequest>,
    ) -> TonicResult<v1::ImageFsInfoResponse> {
        let request = r.into_inner();
        log_object("ImageFsInfo", &request);

        // TODO: Also merge in stats about the upstream system!
        self.0
            .oci_image
            .lock()
            .await
            .image_fs_info(Request::new(request))
            .await
    }
}

impl VimanaCriService {
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
                self.0.get_pod(
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
            self.0
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
                self.0.get_container(
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
            self.0.list_containers(
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
        let result = self.0.oci_runtime.lock().await.list_pod_sandbox(r).await;

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
        let result = self.0.oci_runtime.lock().await.list_containers(r).await;

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
        id: workd_prefix(name),
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
    let id = workd_prefix(name);
    v1::Container {
        id: id.clone(),
        pod_sandbox_id: id,
        metadata: pod.container_metadata.clone(),
        image: None, // TODO
        image_ref: String::from("TODO"),
        state: pod_state_to_cri_container_state(pod.state) as i32,
        created_at: pod.container_created_at,
        labels: pod.container_labels.clone(),
        annotations: pod.container_annotations.clone(),
        image_id: String::from("TODO"),
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
            id: workd_prefix(name),
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
        id: workd_prefix(name),
        metadata: pod.container_metadata.clone(),
        state: pod_state_to_cri_container_state(pod.state) as i32,
        created_at: pod.container_created_at,
        started_at: pod.container_started_at,
        finished_at: pod.container_finished_at,
        exit_code: 0, // TODO: Populate this in case a container fails at runtime.
        image: Some(v1::ImageSpec {
            image: name.component.to_string(),
            // Pre-determined for all Vimana images:
            annotations: HashMap::default(),
            user_specified_image: String::default(),
            runtime_handler: String::from(CONTAINER_RUNTIME_NAME),
        }),
        image_ref: String::from("TODO"),
        reason: String::from("TODO"),
        message: String::from("TODO"),
        labels: pod.container_labels.clone(),
        annotations: pod.container_annotations.clone(),
        // Vimana containers never have volume mounts.
        mounts: Vec::default(),
        // Logging happens entirely via OTLP, not files.
        log_path: String::default(),
        // TODO: Resource limiting information.
        resources: None,
        image_id: String::from("TODO"),
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

fn component_name_from_labels(labels: &HashMap<String, String>) -> Result<ComponentName> {
    ComponentName::new(
        DomainUuid::parse(
            labels
                .get(LABEL_DOMAIN_KEY)
                .ok_or_else(|| Status::invalid_argument("expected-domain-label"))?,
        )?,
        String::from(
            labels
                .get(LABEL_SERVICE_KEY)
                .ok_or_else(|| Status::invalid_argument("expected-service-label"))?,
        ),
        String::from(
            labels
                .get(LABEL_VERSION_KEY)
                .ok_or_else(|| Status::invalid_argument("expected-version-label"))?,
        ),
    )
}

fn log_object<R: Debug>(name: &str, request: R) {
    //eprintln!("[{name}] {request:?}");
}

fn log_object_real<R: Debug>(name: &str, request: R) {
    eprintln!("[{name}] {request:?}");
}
