/// Implementation of the
/// [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
/// for the Work Node runtime.

use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use api_proto::runtime::v1;
use api_proto::runtime::v1::image_service_server::ImageService;
use api_proto::runtime::v1::runtime_service_server::RuntimeService;

const CONTAINER_RUNTIME_NAME: &str = "actio-work";

pub struct ActioCriService {}

/// Type boilerplate for a typical Tonic response result.
type TonicResult<T> = Result<Response<T>, Status>;

#[tonic::async_trait]
impl RuntimeService for ActioCriService {

    async fn version(&self, _r: Request<v1::VersionRequest>) -> TonicResult<v1::VersionResponse> {
        Ok(Response::new(v1::VersionResponse {
            // Version of the kubelet runtime API.
            version: "TODO".to_string(),
            // Name of the container runtime.
            runtime_name: String::from(CONTAINER_RUNTIME_NAME),
            // Version of the container runtime. The string must be semver-compatible.
            runtime_version: "TODO".to_string(),
            // API version of the container runtime. The string must be semver-compatible.
            runtime_api_version: "TODO".to_string(),
        }))
    }

    async fn run_pod_sandbox(&self, _r: Request<v1::RunPodSandboxRequest>) -> TonicResult<v1::RunPodSandboxResponse> {
        // Sandboxing occurs at the container level, not pod. Nothing to do.
        Ok(Response::new(v1::RunPodSandboxResponse::default()))
    }

    async fn stop_pod_sandbox(&self, _r: Request<v1::StopPodSandboxRequest>) -> TonicResult<v1::StopPodSandboxResponse> {
        // Sandboxing occurs at the container level, not pod. Nothing to do.
        Ok(Response::new(v1::StopPodSandboxResponse::default()))
    }

    async fn remove_pod_sandbox(&self, _r: Request<v1::RemovePodSandboxRequest>) -> TonicResult<v1::RemovePodSandboxResponse> {
        // Sandboxing occurs at the container level, not pod. Nothing to do.
        Ok(Response::new(v1::RemovePodSandboxResponse::default()))
    }

    async fn pod_sandbox_status(&self, _r: Request<v1::PodSandboxStatusRequest>) -> TonicResult<v1::PodSandboxStatusResponse> {
        todo!()
    }

    async fn list_pod_sandbox(&self, _r: Request<v1::ListPodSandboxRequest>) -> TonicResult<v1::ListPodSandboxResponse> {
        todo!()
    }

    async fn create_container(&self, _r: Request<v1::CreateContainerRequest>) -> TonicResult<v1::CreateContainerResponse> {
        todo!()
    }

    async fn start_container(&self, _r: Request<v1::StartContainerRequest>) -> TonicResult<v1::StartContainerResponse> {
        todo!()
    }

    async fn stop_container(&self, _r: Request<v1::StopContainerRequest>) -> TonicResult<v1::StopContainerResponse> {
        todo!()
    }

    async fn remove_container(&self, _r: Request<v1::RemoveContainerRequest>) -> TonicResult<v1::RemoveContainerResponse> {
        todo!()
    }

    async fn list_containers(&self, _r: Request<v1::ListContainersRequest>) -> TonicResult<v1::ListContainersResponse> {
        todo!()
    }

    async fn container_status(&self, _r: Request<v1::ContainerStatusRequest>) -> TonicResult<v1::ContainerStatusResponse> {
        todo!()
    }

    async fn update_container_resources(&self, _r: Request<v1::UpdateContainerResourcesRequest>) -> TonicResult<v1::UpdateContainerResourcesResponse> {
        todo!()
    }

    async fn reopen_container_log(&self, _r: Request<v1::ReopenContainerLogRequest>) -> TonicResult<v1::ReopenContainerLogResponse> {
        todo!()
    }

    async fn exec_sync(&self, _r: Request<v1::ExecSyncRequest>) -> TonicResult<v1::ExecSyncResponse> {
        todo!()
    }

    async fn exec(&self, _r: Request<v1::ExecRequest>) -> TonicResult<v1::ExecResponse> {
        todo!()
    }

    async fn attach(&self, _r: Request<v1::AttachRequest>) -> TonicResult<v1::AttachResponse> {
        todo!()
    }

    async fn port_forward(&self, _r: Request<v1::PortForwardRequest>) -> TonicResult<v1::PortForwardResponse> {
        todo!()
    }

    async fn container_stats(&self, _r: Request<v1::ContainerStatsRequest>) -> TonicResult<v1::ContainerStatsResponse> {
        todo!()
    }

    async fn list_container_stats(&self, _r: Request<v1::ListContainerStatsRequest>) -> TonicResult<v1::ListContainerStatsResponse> {
        todo!()
    }

    async fn pod_sandbox_stats(&self, _r: Request<v1::PodSandboxStatsRequest>) -> TonicResult<v1::PodSandboxStatsResponse> {
        todo!()
    }

    async fn list_pod_sandbox_stats(&self, _r: Request<v1::ListPodSandboxStatsRequest>) -> TonicResult<v1::ListPodSandboxStatsResponse> {
        todo!()
    }

    async fn update_runtime_config(&self, _r: Request<v1::UpdateRuntimeConfigRequest>) -> TonicResult<v1::UpdateRuntimeConfigResponse> {
        todo!()
    }

    async fn status(&self, _r: Request<v1::StatusRequest>) -> TonicResult<v1::StatusResponse> {
        todo!()
    }

    async fn checkpoint_container(&self, _r: Request<v1::CheckpointContainerRequest>) -> TonicResult<v1::CheckpointContainerResponse> {
        todo!()
    }

    type GetContainerEventsStream = ReceiverStream<Result<v1::ContainerEventResponse, Status>>;

    async fn get_container_events(&self, _r: Request<v1::GetEventsRequest>) -> TonicResult<Self::GetContainerEventsStream> {
        todo!()
    }

    async fn list_metric_descriptors(&self, _r: Request<v1::ListMetricDescriptorsRequest>) -> TonicResult<v1::ListMetricDescriptorsResponse> {
        todo!()
    }

    async fn list_pod_sandbox_metrics(&self, _r: Request<v1::ListPodSandboxMetricsRequest>) -> TonicResult<v1::ListPodSandboxMetricsResponse> {
        todo!()
    }

    async fn runtime_config(&self, _r: Request<v1::RuntimeConfigRequest>) -> TonicResult<v1::RuntimeConfigResponse> {
        todo!()
    }
}

#[tonic::async_trait]
impl ImageService for ActioCriService {

    async fn list_images(&self, _r: Request<v1::ListImagesRequest>) -> TonicResult<v1::ListImagesResponse> {
        todo!()
    }

    async fn image_status(&self, _r: Request<v1::ImageStatusRequest>) -> TonicResult<v1::ImageStatusResponse> {
        todo!()
    }

    async fn pull_image(&self, _r: Request<v1::PullImageRequest>) -> TonicResult<v1::PullImageResponse> {
        todo!()
    }

    async fn remove_image(&self, _r: Request<v1::RemoveImageRequest>) -> TonicResult<v1::RemoveImageResponse> {
        todo!()
    }

    async fn image_fs_info(&self, _r: Request<v1::ImageFsInfoRequest>) -> TonicResult<v1::ImageFsInfoResponse> {
        todo!()
    }
}
