//! Client used to fetch and compile containers from a registry,
//! caching compiled components and container metadata locally.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use prost::Message;
use reqwest::header::ACCEPT;
use reqwest::{Client, StatusCode as HttpStatusCode};
use serde::Deserialize;
use tokio::task::spawn;
use tonic::Status;
use wasmtime::component::Component;
use wasmtime::Engine as WasmEngine;

use grpc_container_proto::work::runtime::GrpcMetadata;
use names::CanonicalComponentName;

/// Client used to fetch and compile containers from a registry,
/// caching compiled components and container metadata locally.
pub struct ContainerStore {
    client: ContainerClient,
}

impl ContainerStore {
    pub fn new(registry: String, wasmtime: &WasmEngine) -> Self {
        Self {
            client: ContainerClient::new(registry, wasmtime),
        }
    }

    pub async fn get(
        &self,
        name: CanonicalComponentName,
    ) -> Result<(Component, GrpcMetadata), Status> {
        // TODO: Cache metadata / compiled components locally.
        self.client.fetch(name).await
    }
}

/// The container client fetches and processes blobs from a
/// [container registry](https://specs.opencontainers.org/distribution-spec/).
#[derive(Clone)]
struct ContainerClient(Arc<ContainerClientInner>);

struct ContainerClientInner {
    /// Basic HTTP client.
    http: Client,

    /// Scheme, host name, and optional port of the registry (e.g. `http://localhost:5000`).
    registry: String,

    /// Global Wasm engine to run hosted services.
    wasmtime: WasmEngine,
}

impl ContainerClient {
    fn new(registry: String, wasmtime: &WasmEngine) -> Self {
        Self(Arc::new(ContainerClientInner {
            http: Client::new(),
            registry,
            wasmtime: wasmtime.clone(),
        }))
    }

    async fn fetch(
        &self,
        name: CanonicalComponentName,
    ) -> Result<(Component, GrpcMetadata), Status> {
        // Any URL path for `1234567890abcdef1234567890abcdef:package.Service`
        // would begin with `/v2/1234567890abcdef1234567890abcdef/package.Service/`.
        let service_url = format!(
            "{}/v2/{}/{}",
            self.0.registry, name.service.domain, name.service.service,
        );
        // Pull the manifest:
        // https://specs.opencontainers.org/distribution-spec/#pulling-manifests.
        let response = self
            .0
            .http
            .get(format!("{service_url}/manifests/{}", name.version))
            .header(ACCEPT, "manifest.v2+json")
            .send()
            .await
            .map_err(|_| {
                // Fails if there was an error while sending request,
                // redirect loop was detected or redirect limit was exhausted.
                Status::internal("Error fetching container manifest")
            })?;
        if response.status() == HttpStatusCode::OK {
            let manifest = response
                .json::<ImageManifest>()
                .await
                .map_err(|_| Status::internal("Malformed container manifest"))?;
            // All images consist of 2 layers:
            // the component byte code, followed by the serialized metadata.
            if manifest.layers.len() == 2 {
                // Fetch the layers in parallel.
                let component = spawn(self.clone().fetch_component(format!(
                    "{service_url}/blobs/{}",
                    manifest.layers.get(0).unwrap().digest,
                )));
                let metadata = self
                    .fetch_metadata(format!(
                        "{service_url}/blobs/{}",
                        manifest.layers.get(1).unwrap().digest,
                    ))
                    .await;
                let component = component.await.map_err(|_| {
                    // Background task join error.
                    Status::internal("Error fetching component in background")
                })?;
                Ok((component?, metadata?))
            } else {
                Err(Status::internal(format!(
                    "Expected 2 layers in container (got {})",
                    manifest.layers.len()
                )))
            }
        } else if response.status() == HttpStatusCode::NOT_FOUND {
            Err(Status::internal("Container manifest not found"))
        } else {
            Err(Status::internal(format!(
                "Unexpected status from registry while fetching manifest: {}",
                response.status().as_u16()
            )))
        }
    }

    async fn fetch_component(self, url: String) -> Result<Component, Status> {
        let byte_code = self.fetch_blob(url).await?;
        Component::new(&self.0.wasmtime, byte_code)
            .map_err(|_| Status::internal("Component compilation error"))
    }

    async fn fetch_metadata(&self, url: String) -> Result<GrpcMetadata, Status> {
        let serialized = self.fetch_blob(url).await?;
        GrpcMetadata::decode(serialized)
            .map_err(|_| Status::internal("Error decoding Container metadata"))
    }

    async fn fetch_blob(&self, url: String) -> Result<Bytes, Status> {
        let response = self.0.http.get(url).send().await.map_err(|_| {
            // Fails if there was an error while sending request,
            // redirect loop was detected or redirect limit was exhausted.
            Status::internal("Error fetching container blob")
        })?;
        if response.status() == HttpStatusCode::OK {
            response.bytes().await.map_err(|_| {
                // Not sure when this would ever happen.
                Status::internal("Malformed container blob")
            })
        } else if response.status() == HttpStatusCode::NOT_FOUND {
            Err(Status::internal("Container blob not found"))
        } else {
            Err(Status::internal(format!(
                "Unexpected status from registry while fetching blob: {}",
                response.status().as_u16()
            )))
        }
    }
}

/// See [spec](https://specs.opencontainers.org/image-spec/manifest/#image-manifest).
#[derive(Deserialize)]
struct ImageManifest {
    /// This REQUIRED property specifies the image manifest schema version.
    /// For this version of the specification,
    /// this MUST be 2 to ensure backward compatibility with older versions of Docker.
    /// The value of this field will not change.
    /// This field MAY be removed in a future version of the specification.
    schemaVersion: usize,

    /// This property is reserved for use, to maintain compatibility.
    /// When used, this field contains the media type of this document,
    /// which differs from the descriptor use of mediaType.
    #[serde(default)]
    mediaType: String,

    /// This REQUIRED property references a configuration object for a container, by digest.
    ///
    /// Implementations MUST support at least the following media types in the config:
    /// - `application/vnd.oci.image.config.v1+json`
    config: Descriptor,

    /// Each item in the array MUST be a descriptor.
    /// The array MUST have the base layer at index 0.
    /// Subsequent layers MUST then follow in stack order
    /// (i.e. from `layers[0]` to `layers[len(layers)-1]`).
    /// The final filesystem layout
    /// MUST match the result of applying the layers to an empty directory.
    /// The ownership, mode, and other attributes of the initial empty directory are unspecified.
    ///
    /// Implementations MUST support at least the following media types in layers:
    /// - `application/vnd.oci.image.layer.v1.tar`
    /// - `application/vnd.oci.image.layer.v1.tar+gzip`
    /// - `application/vnd.oci.image.layer.nondistributable.v1.tar`
    /// - `application/vnd.oci.image.layer.nondistributable.v1.tar+gzip`
    ///
    /// Entries in this field will frequently use the `+gzip` types.
    layers: Vec<Descriptor>,

    /// This OPTIONAL property contains arbitrary metadata for the image manifest.
    /// This OPTIONAL property MUST use the annotation rules.
    #[serde(default)]
    annotations: HashMap<String, String>,
}

/// See [spec](https://specs.opencontainers.org/image-spec/descriptor/).
#[derive(Deserialize)]
struct Descriptor {
    /// This REQUIRED property contains the media type of the referenced content.
    /// Values MUST comply with RFC 6838, including the naming requirements in its section 4.2.
    /// The OCI image specification defines several of its own MIME types
    /// for resources defined in the specification.
    mediaType: String,

    /// This REQUIRED property is the digest of the targeted content,
    /// conforming to the requirements outlined in Digests.
    /// Retrieved content SHOULD be verified against this digest when consumed via untrusted sources.
    digest: String,

    /// This REQUIRED property specifies the size, in bytes, of the raw content.
    /// This property exists so that a client will have an expected size for the content before processing.
    /// If the length of the retrieved content does not match the specified length,
    /// the content SHOULD NOT be trusted.
    size: u64,

    /// This OPTIONAL property specifies a list of URIs from which this object MAY be downloaded.
    /// Each entry MUST conform to RFC 3986.
    /// Entries SHOULD use the `http` and `https` schemes, as defined in RFC 7230.
    #[serde(default)]
    urls: Vec<String>,

    /// This OPTIONAL property contains arbitrary metadata for this descriptor.
    /// This OPTIONAL property MUST use the annotation rules.
    #[serde(default)]
    annotations: HashMap<String, String>,
}
