//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! `ImageService` for the Work Node runtime.
//!
//! Envoy Gateway and K8s control plane pods expect an OCI-compatible runtime.
//! Since Vimana's Wasm component runtime is not OCI-compatible,
//! Vimana nodes must be tainted such that OCI pods will never run on one.

use std::collections::HashMap;
use std::io::{Error as IoError, ErrorKind};

use anyhow::{Context, Result, anyhow};
use api_proto::runtime::v1;
use api_proto::runtime::v1::image_service_server::ImageService;
use lazy_static::lazy_static;
use regex::Regex;
use tokio_stream::StreamExt;
use tonic::{Request, Response, async_trait};

use crate::containers::ContainerStore;
use crate::cri::{GlobalLogs, LogErrorToStatus, TonicResult};
use crate::state::now;
use names::{ComponentName, DomainUuid};

#[async_trait]
impl ImageService for ContainerStore {
    async fn list_images(
        &self,
        request: Request<v1::ListImagesRequest>,
    ) -> TonicResult<v1::ListImagesResponse> {
        let _image_spec = request
            .into_inner()
            .filter
            .unwrap_or_default()
            .image
            .unwrap_or_default();
        let mut response = v1::ListImagesResponse::default();

        let mut stream = self.scan();
        while let Some(image) = stream.next().await {
            // TODO: Filter these results according to the spec.
            response.images.push(image);
        }

        Ok(Response::new(response))
    }

    async fn image_status(
        &self,
        request: Request<v1::ImageStatusRequest>,
    ) -> TonicResult<v1::ImageStatusResponse> {
        let request = request.into_inner();

        let image_spec = request.image.unwrap_or_default();
        let name = component_from_image_spec(&image_spec.image)
            .context("ImageStatus")
            .log_error(GlobalLogs)?;

        // If the container file was not found,
        // swallow the error and return an empty image,
        // which is what Kubelet expects to indicate that the image must be pulled.
        let image = match self.get_image(&name).await {
            Ok(image) => Some(image),
            Err(error) => {
                if let Some(ErrorKind::NotFound) =
                    error.downcast_ref::<IoError>().map(IoError::kind)
                {
                    None
                } else {
                    return Err(error).log_error(&name);
                }
            }
        };

        Ok(Response::new(v1::ImageStatusResponse {
            image,
            info: HashMap::default(),
        }))
    }

    async fn pull_image(
        &self,
        request: Request<v1::PullImageRequest>,
    ) -> TonicResult<v1::PullImageResponse> {
        let request = request.into_inner();

        let image_spec = request.image.unwrap_or_default();
        let name = component_from_image_spec(&image_spec.image)
            .context("PullImage")
            .log_error(GlobalLogs)?;

        self.pull(&name, &image_spec)
            .await
            .with_context(|| format!("Error pulling image: {:?}", image_spec.image))
            .log_error(&name)?;

        Ok(Response::new(v1::PullImageResponse {
            image_ref: name.to_string(),
        }))
    }

    async fn remove_image(
        &self,
        request: Request<v1::RemoveImageRequest>,
    ) -> TonicResult<v1::RemoveImageResponse> {
        let request = request.into_inner();

        let image_spec = request.image.unwrap_or_default();
        let name = component_from_image_spec(&image_spec.image)
            .context("RemoveImage")
            .log_error(GlobalLogs)?;

        self.remove(&name)
            .await
            .with_context(|| format!("Error removing image: {:?}", image_spec.image))
            .log_error(&name)?;

        Ok(Response::new(v1::RemoveImageResponse {}))
    }

    async fn image_fs_info(
        &self,
        _request: Request<v1::ImageFsInfoRequest>,
    ) -> TonicResult<v1::ImageFsInfoResponse> {
        let usage = self.filesystem_usage().await.log_error(GlobalLogs)?;

        Ok(Response::new(v1::ImageFsInfoResponse {
            image_filesystems: vec![v1::FilesystemUsage {
                timestamp: now(),
                fs_id: Some(v1::FilesystemIdentifier {
                    mountpoint: self.mountpoint(),
                }),
                used_bytes: Some(v1::UInt64Value { value: usage.bytes }),
                inodes_used: Some(v1::UInt64Value {
                    value: usage.inodes,
                }),
            }],
            container_filesystems: Vec::default(),
        }))
    }
}

fn component_from_image_spec(image_id: &str) -> Result<ComponentName> {
    lazy_static! {
        // Use a permissive regex to parse the image ID:
        //     <registry>/<domain-id>/<server-id>:<version>
        static ref IMAGE_ID_RE: Regex = Regex::new(r"^.*/([^/]*)/([^:]*):(.*)$").unwrap();
    }

    let Some(image_id) = IMAGE_ID_RE.captures(image_id) else {
        return Err(anyhow!("Malformed image ID"));
    };
    let domain = &image_id[1];
    let server = &image_id[2];
    let version = &image_id[3];

    ComponentName::new(DomainUuid::parse(domain)?, server, version)
        .with_context(|| format!("Invalid image spec: {:?}", image_id))
}
