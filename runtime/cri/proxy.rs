//! Proxying wrappers to multiplex CRI requests between the Vimana runtime
//! and a downstream OCI runtime (e.g. containerd or cri-o).
//!
//! This enables colocation of Vimana Wasm pods and traditional OCI pods on a single node.
//! Vimana nodes include a taint that blocks most OCI pods.
//! However, kube-proxy and certain cloud-provider-specific pods must run on these nodes anyway.

use std::result::Result as StdResult;
use std::sync::Arc;

use anyhow::{Context, Result};
use cri_api_proto::runtime::v1;
use cri_api_proto::runtime::v1::image_service_client::ImageServiceClient;
use cri_api_proto::runtime::v1::image_service_server::ImageService;
use cri_api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use cri_api_proto::runtime::v1::runtime_service_server::RuntimeService;
use papaya::HashSet as LockFreeConcurrentHashSet;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::spawn;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::channel::Channel;
use tonic::{Request, Status, async_trait};

use super::TonicResult;
use super::runtime::CONTAINER_RUNTIME_HANDLER;
use crate::containers::ContainerStore;
use crate::state::WorkRuntime;
use logging::log_info_globally;

/// Proxying runtime service that routes requests between the Vimana runtime
/// and a downstream OCI runtime.
pub(crate) struct ProxyingRuntimeServer {
    /// The Vimana runtime for Wasm component pods.
    vimana: WorkRuntime,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so Vimana-enabled worker nodes can run traditional OCI containers as well.
    oci_client: Arc<AsyncMutex<RuntimeServiceClient<Channel>>>,

    /// The set of all pod sandbox IDs and container IDs managed by the downstream OCI runtime.
    /// In `containerd`, pod sandbox IDs are just the container ID for the pause container,
    /// so lumping those two seemingly distinct namespaces together makes a degree of sense.
    oci_ids: LockFreeConcurrentHashSet<String>,
}

/// Proxying image service that routes requests between the Vimana runtime
/// and a downstream OCI runtime.
pub(crate) struct ProxyingImageServer {
    /// The Vimana container store for Wasm component images.
    vimana: ContainerStore,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    oci_client: Arc<AsyncMutex<ImageServiceClient<Channel>>>,
}

impl ProxyingRuntimeServer {
    pub async fn new(
        vimana: WorkRuntime,
        mut oci_client: RuntimeServiceClient<Channel>,
    ) -> Result<Self> {
        let oci_ids = LockFreeConcurrentHashSet::new();

        // On startup, list any pre-existing pod sandboxes or containers in the downstream runtime,
        // so requests that reference them can be routed appropriately.
        {
            let downstream_pods = oci_client
                .list_pod_sandbox(Request::new(v1::ListPodSandboxRequest::default()))
                .await
                .context("Failed to list existing pod sandboxes from the downstream runtime")?;
            let pinned = oci_ids.pin();
            for pod in downstream_pods.into_inner().items {
                pinned.insert(pod.id);
            }
        }
        {
            let downstream_containers = oci_client
                .list_containers(Request::new(v1::ListContainersRequest::default()))
                .await
                .context("Failed to list existing containers from the downstream runtime")?;
            let pinned = oci_ids.pin();
            for container in downstream_containers.into_inner().containers {
                pinned.insert(container.id);
            }
        }

        Ok(Self {
            vimana,
            oci_client: Arc::new(AsyncMutex::new(oci_client)),
            oci_ids,
        })
    }

    /// Returns true if the given ID belongs to the downstream OCI runtime.
    fn is_oci(&self, id: &str) -> bool {
        self.oci_ids.pin().contains(id)
    }
}

#[async_trait]
impl RuntimeService for ProxyingRuntimeServer {
    async fn version(
        &self,
        request: Request<v1::VersionRequest>,
    ) -> TonicResult<v1::VersionResponse> {
        RuntimeService::version(&self.vimana, request).await
    }

    async fn run_pod_sandbox(
        &self,
        request: Request<v1::RunPodSandboxRequest>,
    ) -> TonicResult<v1::RunPodSandboxResponse> {
        // Unless `vimana-handler` is explicitly chosen,
        // forward requests to the downstream OCI runtime.
        if request.get_ref().runtime_handler != CONTAINER_RUNTIME_HANDLER {
            if let Some(config) = &request.get_ref().config
                && let Some(metadata) = &config.metadata
            {
                log_info_globally!("Proxying RunPodSandboxRequest for {:?}", metadata.name);
            } else {
                log_info_globally!("Proxying RunPodSandboxRequest");
            }
            let response = self.oci_client.lock().await.run_pod_sandbox(request).await;
            if let Ok(reply) = &response {
                self.oci_ids
                    .pin()
                    .insert(reply.get_ref().pod_sandbox_id.clone());
            }
            return response;
        }

        RuntimeService::run_pod_sandbox(&self.vimana, request).await
    }

    async fn stop_pod_sandbox(
        &self,
        request: Request<v1::StopPodSandboxRequest>,
    ) -> TonicResult<v1::StopPodSandboxResponse> {
        let pod_sandbox_id = &request.get_ref().pod_sandbox_id;
        if self.is_oci(pod_sandbox_id) {
            log_info_globally!("Proxying StopPodSandbox for {:?}", pod_sandbox_id);
            return self.oci_client.lock().await.stop_pod_sandbox(request).await;
        }

        RuntimeService::stop_pod_sandbox(&self.vimana, request).await
    }

    async fn remove_pod_sandbox(
        &self,
        request: Request<v1::RemovePodSandboxRequest>,
    ) -> TonicResult<v1::RemovePodSandboxResponse> {
        let pod_sandbox_id = &request.get_ref().pod_sandbox_id;
        if self.is_oci(pod_sandbox_id) {
            log_info_globally!("Proxying RemovePodSandbox for {:?}", pod_sandbox_id);
            let pod_sandbox_id = pod_sandbox_id.clone();
            let response = self
                .oci_client
                .lock()
                .await
                .remove_pod_sandbox(request)
                .await;
            if response.is_ok() {
                self.oci_ids.pin().remove(&pod_sandbox_id);
            }
            return response;
        }

        RuntimeService::remove_pod_sandbox(&self.vimana, request).await
    }

    async fn pod_sandbox_status(
        &self,
        request: Request<v1::PodSandboxStatusRequest>,
    ) -> TonicResult<v1::PodSandboxStatusResponse> {
        let pod_sandbox_id = &request.get_ref().pod_sandbox_id;
        if self.is_oci(pod_sandbox_id) {
            log_info_globally!("Proxying PodSandboxStatus for {:?}", pod_sandbox_id);
            return self
                .oci_client
                .lock()
                .await
                .pod_sandbox_status(request)
                .await;
        }

        RuntimeService::pod_sandbox_status(&self.vimana, request).await
    }

    async fn list_pod_sandbox(
        &self,
        request: Request<v1::ListPodSandboxRequest>,
    ) -> TonicResult<v1::ListPodSandboxResponse> {
        // Combine results from both runtimes.
        let oci_client = self.oci_client.clone();
        let oci_request = Request::new(request.get_ref().clone());
        let oci_response =
            spawn(async move { oci_client.lock().await.list_pod_sandbox(oci_request).await });

        let mut response = RuntimeService::list_pod_sandbox(&self.vimana, request).await?;

        response
            .get_mut()
            .items
            .append(&mut oci_response.await.unwrap()?.into_inner().items);

        Ok(response)
    }

    async fn create_container(
        &self,
        request: Request<v1::CreateContainerRequest>,
    ) -> TonicResult<v1::CreateContainerResponse> {
        let pod_sandbox_id = &request.get_ref().pod_sandbox_id;
        if self.is_oci(pod_sandbox_id) {
            log_info_globally!("Proxying CreateContainer for {:?}", pod_sandbox_id);
            let response = self.oci_client.lock().await.create_container(request).await;
            if let Ok(reply) = &response {
                self.oci_ids
                    .pin()
                    .insert(reply.get_ref().container_id.clone());
            }
            return response;
        }

        RuntimeService::create_container(&self.vimana, request).await
    }

    async fn start_container(
        &self,
        request: Request<v1::StartContainerRequest>,
    ) -> TonicResult<v1::StartContainerResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying StartContainer for {:?}", container_id);
            return self.oci_client.lock().await.start_container(request).await;
        }

        RuntimeService::start_container(&self.vimana, request).await
    }

    async fn stop_container(
        &self,
        request: Request<v1::StopContainerRequest>,
    ) -> TonicResult<v1::StopContainerResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying StopContainer for {:?}", container_id);
            return self.oci_client.lock().await.stop_container(request).await;
        }

        RuntimeService::stop_container(&self.vimana, request).await
    }

    async fn remove_container(
        &self,
        request: Request<v1::RemoveContainerRequest>,
    ) -> TonicResult<v1::RemoveContainerResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying RemoveContainer for {:?}", container_id);
            let container_id = container_id.clone();
            let response = self.oci_client.lock().await.remove_container(request).await;
            if response.is_ok() {
                self.oci_ids.pin().remove(&container_id);
            }
            return response;
        }

        RuntimeService::remove_container(&self.vimana, request).await
    }

    async fn list_containers(
        &self,
        request: Request<v1::ListContainersRequest>,
    ) -> TonicResult<v1::ListContainersResponse> {
        // Combine results from both runtimes.
        let oci_client = self.oci_client.clone();
        let oci_request = Request::new(request.get_ref().clone());
        let oci_response =
            spawn(async move { oci_client.lock().await.list_containers(oci_request).await });

        let mut response = RuntimeService::list_containers(&self.vimana, request).await?;

        response
            .get_mut()
            .containers
            .append(&mut oci_response.await.unwrap()?.into_inner().containers);

        Ok(response)
    }

    async fn container_status(
        &self,
        request: Request<v1::ContainerStatusRequest>,
    ) -> TonicResult<v1::ContainerStatusResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying ContainerStatus for {:?}", container_id);
            return self.oci_client.lock().await.container_status(request).await;
        }

        RuntimeService::container_status(&self.vimana, request).await
    }

    async fn update_container_resources(
        &self,
        request: Request<v1::UpdateContainerResourcesRequest>,
    ) -> TonicResult<v1::UpdateContainerResourcesResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying UpdateContainerResources for {:?}", container_id);
            return self
                .oci_client
                .lock()
                .await
                .update_container_resources(request)
                .await;
        }

        RuntimeService::update_container_resources(&self.vimana, request).await
    }

    async fn reopen_container_log(
        &self,
        request: Request<v1::ReopenContainerLogRequest>,
    ) -> TonicResult<v1::ReopenContainerLogResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying ReopenContainerLog for {:?}", container_id);
            return self
                .oci_client
                .lock()
                .await
                .reopen_container_log(request)
                .await;
        }

        RuntimeService::reopen_container_log(&self.vimana, request).await
    }

    async fn exec_sync(
        &self,
        request: Request<v1::ExecSyncRequest>,
    ) -> TonicResult<v1::ExecSyncResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying ExecSync for {:?}", container_id);
            return self.oci_client.lock().await.exec_sync(request).await;
        }

        RuntimeService::exec_sync(&self.vimana, request).await
    }

    async fn exec(&self, request: Request<v1::ExecRequest>) -> TonicResult<v1::ExecResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying Exec for {:?}", container_id);
            return self.oci_client.lock().await.exec(request).await;
        }

        RuntimeService::exec(&self.vimana, request).await
    }

    async fn attach(&self, request: Request<v1::AttachRequest>) -> TonicResult<v1::AttachResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying Attach for {:?}", container_id);
            return self.oci_client.lock().await.attach(request).await;
        }

        RuntimeService::attach(&self.vimana, request).await
    }

    async fn port_forward(
        &self,
        request: Request<v1::PortForwardRequest>,
    ) -> TonicResult<v1::PortForwardResponse> {
        let pod_sandbox_id = &request.get_ref().pod_sandbox_id;
        if self.is_oci(pod_sandbox_id) {
            log_info_globally!("Proxying PortForward for {:?}", pod_sandbox_id);
            return self.oci_client.lock().await.port_forward(request).await;
        }

        RuntimeService::port_forward(&self.vimana, request).await
    }

    async fn container_stats(
        &self,
        request: Request<v1::ContainerStatsRequest>,
    ) -> TonicResult<v1::ContainerStatsResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying ContainerStats for {:?}", container_id);
            return self.oci_client.lock().await.container_stats(request).await;
        }

        RuntimeService::container_stats(&self.vimana, request).await
    }

    async fn list_container_stats(
        &self,
        request: Request<v1::ListContainerStatsRequest>,
    ) -> TonicResult<v1::ListContainerStatsResponse> {
        // TODO: Incoporate a Vimana implementation of this.
        self.oci_client
            .lock()
            .await
            .list_container_stats(request)
            .await
    }

    async fn pod_sandbox_stats(
        &self,
        request: Request<v1::PodSandboxStatsRequest>,
    ) -> TonicResult<v1::PodSandboxStatsResponse> {
        let pod_sandbox_id = &request.get_ref().pod_sandbox_id;
        if self.is_oci(pod_sandbox_id) {
            log_info_globally!("Proxying ContainerStats for {:?}", pod_sandbox_id);
            return self
                .oci_client
                .lock()
                .await
                .pod_sandbox_stats(request)
                .await;
        }

        RuntimeService::pod_sandbox_stats(&self.vimana, request).await
    }

    async fn list_pod_sandbox_stats(
        &self,
        request: Request<v1::ListPodSandboxStatsRequest>,
    ) -> TonicResult<v1::ListPodSandboxStatsResponse> {
        // TODO: Incoporate a Vimana implementation of this.
        self.oci_client
            .lock()
            .await
            .list_pod_sandbox_stats(request)
            .await
    }

    async fn update_runtime_config(
        &self,
        request: Request<v1::UpdateRuntimeConfigRequest>,
    ) -> TonicResult<v1::UpdateRuntimeConfigResponse> {
        // Update both runtimes.
        let oci_client = self.oci_client.clone();
        let oci_request = Request::new(request.get_ref().clone());
        let oci_response = spawn(async move {
            oci_client
                .lock()
                .await
                .update_runtime_config(oci_request)
                .await
        });

        RuntimeService::update_runtime_config(&self.vimana, request).await?;

        // `UpdateRuntimeConfigResponse` is an empty struct.
        oci_response.await.unwrap()
    }

    async fn status(&self, request: Request<v1::StatusRequest>) -> TonicResult<v1::StatusResponse> {
        // TODO: Incoporate the downstream response to this.
        RuntimeService::status(&self.vimana, request).await
    }

    async fn checkpoint_container(
        &self,
        request: Request<v1::CheckpointContainerRequest>,
    ) -> TonicResult<v1::CheckpointContainerResponse> {
        let container_id = &request.get_ref().container_id;
        if self.is_oci(container_id) {
            log_info_globally!("Proxying CheckpointContainer for {:?}", container_id);
            return self
                .oci_client
                .lock()
                .await
                .checkpoint_container(request)
                .await;
        }

        RuntimeService::checkpoint_container(&self.vimana, request).await
    }

    type GetContainerEventsStream = ReceiverStream<StdResult<v1::ContainerEventResponse, Status>>;

    async fn get_container_events(
        &self,
        request: Request<v1::GetEventsRequest>,
    ) -> TonicResult<Self::GetContainerEventsStream> {
        // TODO: Incoporate a Vimana implementation of this.
        self.vimana.get_container_events(request).await
    }

    async fn list_metric_descriptors(
        &self,
        request: Request<v1::ListMetricDescriptorsRequest>,
    ) -> TonicResult<v1::ListMetricDescriptorsResponse> {
        // TODO: Incoporate a Vimana implementation of this.
        self.oci_client
            .lock()
            .await
            .list_metric_descriptors(request)
            .await
    }

    async fn list_pod_sandbox_metrics(
        &self,
        request: Request<v1::ListPodSandboxMetricsRequest>,
    ) -> TonicResult<v1::ListPodSandboxMetricsResponse> {
        // TODO: Incoporate a Vimana implementation of this.
        self.oci_client
            .lock()
            .await
            .list_pod_sandbox_metrics(request)
            .await
    }

    async fn runtime_config(
        &self,
        request: Request<v1::RuntimeConfigRequest>,
    ) -> TonicResult<v1::RuntimeConfigResponse> {
        // The only information in the response is the "Cgroup driver to use".
        // Since this is only relevant to the OCI runtime, defer to it.
        self.oci_client.lock().await.runtime_config(request).await
    }

    async fn update_pod_sandbox_resources(
        &self,
        request: Request<v1::UpdatePodSandboxResourcesRequest>,
    ) -> TonicResult<v1::UpdatePodSandboxResourcesResponse> {
        // TODO: Incoporate a Vimana implementation of this.
        self.vimana.update_pod_sandbox_resources(request).await
    }
}

impl ProxyingImageServer {
    pub fn new(vimana: ContainerStore, oci_client: ImageServiceClient<Channel>) -> Self {
        Self {
            vimana,
            oci_client: Arc::new(AsyncMutex::new(oci_client)),
        }
    }

    /// Returns true if the given image spec belongs to the downstream OCI runtime.
    fn is_oci(image: &Option<v1::ImageSpec>) -> bool {
        image
            .as_ref()
            .is_none_or(|spec| spec.runtime_handler != CONTAINER_RUNTIME_HANDLER)
    }
}

#[async_trait]
impl ImageService for ProxyingImageServer {
    async fn list_images(
        &self,
        request: Request<v1::ListImagesRequest>,
    ) -> TonicResult<v1::ListImagesResponse> {
        // Combine results from both runtimes.
        let oci_client = self.oci_client.clone();
        let oci_request = Request::new(request.get_ref().clone());
        let oci_response =
            spawn(async move { oci_client.lock().await.list_images(oci_request).await });

        let mut response = ImageService::list_images(&self.vimana, request).await?;

        response
            .get_mut()
            .images
            .append(&mut oci_response.await.unwrap()?.into_inner().images);

        Ok(response)
    }

    async fn image_status(
        &self,
        request: Request<v1::ImageStatusRequest>,
    ) -> TonicResult<v1::ImageStatusResponse> {
        let image = &request.get_ref().image;
        if ProxyingImageServer::is_oci(image) {
            if let Some(image) = image {
                log_info_globally!("Proxying ImageStatus for {:?}", image.image);
            } else {
                log_info_globally!("Proxying ImageStatus");
            }
            return self.oci_client.lock().await.image_status(request).await;
        }

        ImageService::image_status(&self.vimana, request).await
    }

    async fn pull_image(
        &self,
        request: Request<v1::PullImageRequest>,
    ) -> TonicResult<v1::PullImageResponse> {
        let image = &request.get_ref().image;
        if ProxyingImageServer::is_oci(image) {
            if let Some(image) = image {
                log_info_globally!("Proxying PullImage for {:?}", image.image);
            } else {
                log_info_globally!("Proxying PullImage");
            }
            return self.oci_client.lock().await.pull_image(request).await;
        }

        ImageService::pull_image(&self.vimana, request).await
    }

    async fn remove_image(
        &self,
        request: Request<v1::RemoveImageRequest>,
    ) -> TonicResult<v1::RemoveImageResponse> {
        let image = &request.get_ref().image;
        if ProxyingImageServer::is_oci(image) {
            if let Some(image) = image {
                log_info_globally!("Proxying RemoveImage for {:?}", image.image);
            } else {
                log_info_globally!("Proxying RemoveImage");
            }
            return self.oci_client.lock().await.remove_image(request).await;
        }

        ImageService::remove_image(&self.vimana, request).await
    }

    async fn image_fs_info(
        &self,
        request: Request<v1::ImageFsInfoRequest>,
    ) -> TonicResult<v1::ImageFsInfoResponse> {
        // Combine results from both runtimes.
        let oci_client = self.oci_client.clone();
        let oci_request = Request::new(*request.get_ref());
        let oci_response =
            spawn(async move { oci_client.lock().await.image_fs_info(oci_request).await });

        let mut response = ImageService::image_fs_info(&self.vimana, request).await?;

        response
            .get_mut()
            .image_filesystems
            .append(&mut oci_response.await.unwrap()?.into_inner().image_filesystems);

        Ok(response)
    }
}
