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
use std::result::Result as StdResult;
use std::sync::Arc;

use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status};

use api_proto::runtime::v1;
use api_proto::runtime::v1::image_service_server::ImageService;
use api_proto::runtime::v1::runtime_service_server::RuntimeService;
use api_proto::runtime::v1::Protocol;

use error::{Error, Result};
use names::{CanonicalComponentName, CanonicalDomainUuid, Name, PodName};
use state::WorkRuntime;

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

const HTTPS_DEFAULT_PORT: i32 = 443;

/// Wrapper around [WorkRuntime] that can implement [RuntimeService] and [ImageService]
/// without running afoul of Rust's rules on foreign types / traits.
pub struct VimanaCriService(pub Arc<WorkRuntime>);

/// Type boilerplate for a typical Tonic response result.
type TonicResult<T> = StdResult<Response<T>, Status>;

/// Return early with the result of the given block
/// if the given ID (mutable `String`) starts with the OCI prefix.
/// Otherwise, assume it starts with the word prefix and continue.
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
        $id = String::from(&id_value[WORKD_PREFIX.len()..]);
    };
}

#[inline]
fn oci_prefix<S: Display>(id: S) -> String {
    format!("{OCI_PREFIX}{}", id)
}

#[inline]
fn workd_prefix<S: Display>(id: S) -> String {
    format!("{WORKD_PREFIX}{}", id)
}

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

fn log_object<R: Debug>(name: &str, request: R) {
    eprintln!("[{name}] {request:?}");
}

#[tonic::async_trait]
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

        let mut config = request.config.unwrap_or_default();
        let component = component_name_from_labels(config.labels)?;

        // Check that the request fits into Vimana's narrow vision of validity
        // for the sake of preventing unexpected behavior.
        if config.port_mappings.len() != 1 {
            return Err(Error::leaf("gRPC pods must have a single port mapping.").into());
        }
        let port_mapping = config.port_mappings.pop().unwrap();
        if port_mapping.protocol != Protocol::Tcp as i32 {
            return Err(Error::leaf("gRPC pods must use TCP.").into());
        }
        if port_mapping.container_port != HTTPS_DEFAULT_PORT {
            return Err(Error::leaf(format!(
                "gRPC pods must use internal port {}.",
                HTTPS_DEFAULT_PORT
            ))
            .into());
        }
        if port_mapping.host_port <= 0 || port_mapping.host_port > u16::MAX as i32 {
            return Err(Error::leaf(format!(
                "Invalid external port '{}'.",
                port_mapping.host_port
            ))
            .into());
        }

        let pod_id = self.0.init_pod(
            config.metadata.unwrap_or_default(),
            component.clone(),
            port_mapping.host_port as u16,
        )?;

        Ok(Response::new(v1::RunPodSandboxResponse {
            // Prefix the ID so we can distinguish it from downstream OCI pod IDs.
            pod_sandbox_id: workd_prefix(PodName::new(component, pod_id)),
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

        todo!()
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

        todo!()
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
                // TODO: Also intercept and prefix container IDs.
                .map(map_oci_prefix!(status, id))
        });

        todo!()
    }

    async fn list_pod_sandbox(
        &self,
        r: Request<v1::ListPodSandboxRequest>,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        let mut request = r.into_inner();
        log_object("ListPodSandbox", &request);

        // If there's a filter ID for a given runtime, use it.
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

        // Otherwise, we have to combine the results with the downstream runtime.
        self.list_pod_sandbox_downstream(Request::new(request.clone()))
            .await
            .and_then(|mut downstream_result| {
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
        log_object("CreateContainer", &request);

        intercept_prefix!(request.pod_sandbox_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .create_container(Request::new(request))
                .await
                .map(map_oci_prefix!(container_id))
        });

        let pod_name = Name::parse(&request.pod_sandbox_id).pod()?;
        let config = request.config.unwrap_or_default();
        let component = component_name_from_labels(config.labels)?;

        // While redundant, the component name from the container's labels
        // must match the component name extracted from the pod ID and image ID.
        if component != pod_name.component {
            return Err(Error::leaf("Container labels must match pod sandbox ID.").into());
        }
        let image_id = config.image.unwrap_or_default().image;
        let image_component = Name::parse(&image_id).canonical_component()?;
        if component != image_component {
            return Err(Error::leaf("Container labels must match image ID.").into());
        }

        let mut environment = HashMap::with_capacity(config.envs.len());
        for key_value in config.envs.iter() {
            let pre_existing = environment.insert(key_value.key.clone(), key_value.value.clone());
            debug_assert!(pre_existing.is_none());
        }

        // The CRI API has separate steps for creating pods and creating containers,
        // but a component pod is inseparable from its single container,
        // so "pods" and containers are created simultaneously.
        self.0.create_container(pod_name.pod, environment)?;

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

        todo!()
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

        todo!()
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

        todo!()
    }

    async fn list_containers(
        &self,
        r: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        let mut request = r.into_inner();
        log_object("ListContainers", &request);

        // If there's a filter ID for a given runtime, use it.
        if let Some(ref mut filter) = &mut request.filter {
            if filter.id.starts_with(OCI_PREFIX) {
                filter.id = String::from(&filter.id[OCI_PREFIX.len()..]);
                return self.list_containers_downstream(Request::new(request)).await;
            } else if filter.id.starts_with(WORKD_PREFIX) {
                filter.id = String::from(&filter.id[WORKD_PREFIX.len()..]);
                return self.list_containers_upstream(request);
            }
        }

        // Otherwise, we have to combine the results with the downstream runtime.
        self.list_containers_downstream(Request::new(request.clone()))
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
            })
    }

    async fn container_status(
        &self,
        r: Request<v1::ContainerStatusRequest>,
    ) -> TonicResult<v1::ContainerStatusResponse> {
        let mut request = r.into_inner();
        log_object("ContainerStatus", &request);

        intercept_prefix!(request.container_id, {
            self.0
                .oci_runtime
                .lock()
                .await
                .container_status(Request::new(request))
                .await
                .map(map_oci_prefix!(status, id))
        });

        todo!()
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

        // TODO: Also merge in stats about the upstream system!
        self.0
            .oci_runtime
            .lock()
            .await
            .status(Request::new(request))
            .await
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

    type GetContainerEventsStream = ReceiverStream<StdResult<v1::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        r: Request<v1::GetEventsRequest>,
    ) -> TonicResult<Self::GetContainerEventsStream> {
        let request = r.into_inner();
        log_object("GetContainerEvents", &request);

        // TODO: Figure out how streaming works.
        return Err(Error::leaf("GetContainerEvents TODO").into());
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

#[tonic::async_trait]
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
            let r = self
                .0
                .oci_image
                .lock()
                .await
                .list_images(Request::new(request))
                .await;
            log_object("ListImages-response", &r);
            return r;
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
        let r = self
            .0
            .oci_image
            .lock()
            .await
            .image_fs_info(Request::new(request))
            .await;
        log_object("ImageFsInfo-response", &r);
        r
    }
}

impl VimanaCriService {
    /// Perform sandbox listing in the workd runtime.
    fn list_pod_sandbox_upstream(
        &self,
        _r: v1::ListPodSandboxRequest,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        // TODO: Something real.
        Ok(Response::new(v1::ListPodSandboxResponse::default()))
    }

    /// Perform sandbox listing in the workd runtime.
    fn list_containers_upstream(
        &self,
        _r: v1::ListContainersRequest,
    ) -> TonicResult<v1::ListContainersResponse> {
        // TODO: Something real.
        Ok(Response::new(v1::ListContainersResponse::default()))
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

// These labels must be present on every pod and container using the Vimana handler.
const LABEL_DOMAIN_KEY: &str = "vimana.host/domain";
const LABEL_SERVICE_KEY: &str = "vimana.host/service";
const LABEL_VERSION_KEY: &str = "vimana.host/version";

fn component_name_from_labels(labels: HashMap<String, String>) -> Result<CanonicalComponentName> {
    CanonicalComponentName::new(
        CanonicalDomainUuid::parse(
            labels
                .get(LABEL_DOMAIN_KEY)
                .ok_or_else(|| Error::leaf(format!("Expected label for '{LABEL_DOMAIN_KEY}'")))?,
        )?,
        String::from(
            labels
                .get(LABEL_SERVICE_KEY)
                .ok_or_else(|| Error::leaf(format!("Expected label for '{LABEL_SERVICE_KEY}'")))?,
        ),
        String::from(
            labels
                .get(LABEL_VERSION_KEY)
                .ok_or_else(|| Error::leaf(format!("Expected label for '{LABEL_VERSION_KEY}'")))?,
        ),
    )
}
