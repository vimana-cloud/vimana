//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! `ImageService` for the Work Node runtime.
//!
//! K8s control plane pods expect an OCI-compatible runtime.
//! Since Vimana's Wasm component runtime is not OCI-compatible,
//! this implementation relies on a downstream OCI runtime to run control plane pods,
//! enabling colocation of pods using diverse runtimes on a single node.

use std::collections::HashMap;

use api_proto::runtime::v1;
use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::image_service_server::ImageService;
use api_proto::runtime::v1::ImageSpec;
use lazy_static::lazy_static;
use regex::Regex;
use tokio::sync::Mutex as AsyncMutex;
use tonic::transport::channel::Channel;
use tonic::{async_trait, Request, Response, Status};

use crate::containers::ContainerStore;
use crate::cri::runtime::CONTAINER_RUNTIME_NAME;
use crate::cri::{component_name_from_labels, TonicResult};
use error::Result;
use names::{unhexify_string, ComponentName, DomainUuid};

/// Wrapper around [WorkRuntime] that implements [ImageService]
/// with a downstream server for OCI requests.
pub(crate) struct ProxyingImageService {
    /// The upstream runtime handler for all Vimana-related business logic.
    containers: ContainerStore,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    oci_image: AsyncMutex<ImageServiceClient<Channel>>,
}

#[async_trait]
impl ImageService for ProxyingImageService {
    async fn list_images(
        &self,
        r: Request<v1::ListImagesRequest>,
    ) -> TonicResult<v1::ListImagesResponse> {
        let request = r.into_inner();

        let filter = request.clone().filter.unwrap_or_default();
        let image_spec = filter.image.unwrap_or_default();
        let handler = image_spec.runtime_handler;

        // Unless workd is explicitly chosen, forward all requests to the downstream OCI runtime.
        // This supports running K8s control plane pods like `kube-controller-manager` etc.
        if handler != "TODO-this-should-be-something-else-but-what?" {
            return self
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

        if let Some(image_spec) = &request.image {
            // Fall back on the downstream runtime for non-Vimana images.
            if image_spec.runtime_handler != CONTAINER_RUNTIME_NAME {
                return self
                    .oci_image
                    .lock()
                    .await
                    .image_status(Request::new(request))
                    .await;
            }
        }

        let image_spec = request.image.unwrap_or_default();
        let (registry, name) = registry_and_component_from_image_spec(&image_spec)?;

        let image = self.containers.get_image(&name).await?;

        Ok(Response::new(v1::ImageStatusResponse {
            image: Some(image),
            info: HashMap::default(),
        }))
    }

    async fn pull_image(
        &self,
        r: Request<v1::PullImageRequest>,
    ) -> TonicResult<v1::PullImageResponse> {
        let request = r.into_inner();

        if let Some(image_spec) = &request.image {
            // Fall back on the downstream runtime for non-Vimana images.
            if image_spec.runtime_handler != CONTAINER_RUNTIME_NAME {
                return self
                    .oci_image
                    .lock()
                    .await
                    .pull_image(Request::new(request))
                    .await;
            }
        }

        let image_spec = request.image.unwrap_or_default();
        let (registry, name) = registry_and_component_from_image_spec(&image_spec)?;

        // Invariant check:
        // make sure the component name from the image ID matches that from the pod's labels.
        if name != component_name_from_labels(&request.sandbox_config.unwrap_or_default().labels)? {
            return Err(Status::invalid_argument("pull-image-labels-mismatch"));
        }

        self.containers.pull(&registry, &name, image_spec).await?;

        Ok(Response::new(v1::PullImageResponse {
            image_ref: name.to_string(),
        }))
    }

    async fn remove_image(
        &self,
        r: Request<v1::RemoveImageRequest>,
    ) -> TonicResult<v1::RemoveImageResponse> {
        let request = r.into_inner();

        if let Some(image_spec) = &request.image {
            // Fall back on the downstream runtime for non-Vimana images.
            if image_spec.runtime_handler != CONTAINER_RUNTIME_NAME {
                return self
                    .oci_image
                    .lock()
                    .await
                    .remove_image(Request::new(request))
                    .await;
            }
        }

        todo!()
    }

    async fn image_fs_info(
        &self,
        r: Request<v1::ImageFsInfoRequest>,
    ) -> TonicResult<v1::ImageFsInfoResponse> {
        let request = r.into_inner();

        // TODO: Also merge in stats about the upstream system!
        self.oci_image
            .lock()
            .await
            .image_fs_info(Request::new(request))
            .await
    }
}

impl ProxyingImageService {
    pub(crate) fn new(containers: ContainerStore, oci_image: ImageServiceClient<Channel>) -> Self {
        Self {
            containers,
            oci_image: AsyncMutex::new(oci_image),
        }
    }
}

fn registry_and_component_from_image_spec(image: &ImageSpec) -> Result<(String, ComponentName)> {
    lazy_static! {
        // Use a permissive regex to parse the image ID:
        //     <registry>/<domain>/<service-hex>:<version>
        static ref IMAGE_ID_RE: Regex = Regex::new(r"^([^/]*)/([^/]*)/([^:]*):(.*)$").unwrap();
    }

    let Some(image_id) = IMAGE_ID_RE.captures(&image.image) else {
        return Err(Status::internal("invalid-image-id"));
    };
    let registry = &image_id[1];
    let domain = &image_id[2];
    let service_hex = &image_id[3];
    let version = &image_id[4];

    let name = ComponentName::new(
        DomainUuid::parse(domain)?,
        unhexify_string(service_hex)?,
        version,
    )?;
    Ok((String::from(registry), name))
}
