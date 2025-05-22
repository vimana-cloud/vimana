//! Client used to fetch and compile containers from a registry,
//! caching compiled components and container metadata locally.

use std::collections::{HashMap, HashSet};
use std::mem::drop;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use api_proto::runtime::v1;
use bytes::Bytes;
use prost::Message;
use reqwest::header::ACCEPT;
use reqwest::{Client, StatusCode as HttpStatusCode};
use serde::Deserialize;
use tokio::fs::{create_dir_all, metadata, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::spawn;
use wasmtime::component::Component;
use wasmtime::Engine as WasmEngine;

use logging::log_info;
use metadata_proto::work::runtime::Metadata;
use names::{hexify_string, ComponentName};

/// Path to the root of the file hierarchy for pulled images.
/// See [component_path].
const STORE_ROOT: &str = "/etc/workd/images";

/// Each image directory under [STORE_ROOT] has a file called `container`
/// containing the pre-compiled [Component] and the [Metadata].
const CONTAINER_FILENAME: &str = "container";

/// Each image directory under [STORE_ROOT] also has a file called `image-spec.binpb`
/// containing the [image spec](v1::ImageSpec)
/// that was originally specified when pulling the image.
const IMAGE_SPEC_FILENAME: &str = "image-spec.binpb";

/// Client used to fetch and compile containers from a registry,
/// caching compiled components and parsed container metadata locally.
#[derive(Clone)]
pub(crate) struct ContainerStore {
    /// Means to fetch containers from a remote container registry.
    client: ContainerClient,

    /// Global Wasm engine to run hosted services.
    /// This must be the exact same engine used in the [client](ContainerClient).
    wasmtime: WasmEngine,
}

/// Ready-to-link container.
pub(crate) struct Container {
    /// Compiled component implementation.
    pub(crate) component: Component,

    /// Parsed container metadata.
    pub(crate) metadata: Metadata,
}

impl ContainerStore {
    /// Return a new [store](ContainerStore).
    /// Components will be instantiated using the provided [`Engine`](WasmEngine).
    pub(crate) fn new(insecure_registries: HashSet<String>, wasmtime: &WasmEngine) -> Self {
        Self {
            client: ContainerClient::new(insecure_registries, wasmtime),
            wasmtime: wasmtime.clone(),
        }
    }

    /// Fetch a container identified by `name` from the given registry.
    /// Subsequent calls to `get` should succeed for that container.
    pub(crate) async fn pull(
        &self,
        registry: &str,
        name: &ComponentName,
        image_spec: &v1::ImageSpec,
    ) -> Result<()> {
        let container = self.client.fetch(registry, name).await?;

        let component_path = component_path(&name);
        create_dir_all(&component_path)
            .await
            .with_context(|| format!("Failed to create image directory: {:?}", component_path))?;

        let mut container_file = File::create(&component_path.join(CONTAINER_FILENAME))
            .await
            .context("Failed to create container file")?;
        let mut image_spec_file = File::create(&component_path.join(IMAGE_SPEC_FILENAME))
            .await
            .context("Failed to create image spec file")?;

        // Pre-compile the component for faster loading later.
        // TODO: Prefer to use wasmtime's `Engine::precompile_component`.
        let serialized_component = container.component.serialize()?;

        // Write the length of the component to the file first (as a little-endian `u64`),
        // then the component's pre-compiled bytes.
        container_file
            .write_u64_le(serialized_component.len() as u64)
            .await
            .context("Failed writing length to container file")?;
        container_file
            .write_all(&serialized_component)
            .await
            .context("Failed writing component to container file")?;
        // Free space as soon as possible since it could be big.
        drop(serialized_component);

        let serialized_metadata = container.metadata.encode_to_vec();
        container_file
            .write_all(&serialized_metadata)
            .await
            .context("Failed writing metadata to container file")?;
        drop(serialized_metadata);

        let serialized_image_spec = image_spec.encode_to_vec();
        image_spec_file
            .write_all(&serialized_image_spec)
            .await
            .context("Failed writing to image spec file")?;

        Ok(())
    }

    /// Return a compiled component implementation and its metadata.
    pub(crate) async fn get(&self, name: &ComponentName) -> Result<Container> {
        let component_path = component_path(&name);

        let mut container_file = File::open(&component_path.join(CONTAINER_FILENAME))
            .await
            .context("Failed to open container file")?;

        let component_size = container_file
            .read_u64_le()
            .await
            .context("Failed reading length from container file")?;
        let mut serialized_component = Vec::with_capacity(component_size as usize);
        container_file
            .read_exact(&mut serialized_component)
            .await
            .context("Failed reading component from container file")?;
        let component = unsafe { Component::deserialize(&self.wasmtime, &serialized_component)? };
        // Free space as soon as possible since it could be big.
        drop(serialized_component);

        let mut serialized_metadata = Vec::new();
        container_file
            .read_to_end(&mut serialized_metadata)
            .await
            .context("Failed reading metadata from container file")?;
        let metadata = Metadata::decode(serialized_metadata.as_slice())
            .context("Failed to decode metadata from container file")?;

        Ok(Container {
            component,
            metadata,
        })
    }

    /// Return metadata about the image originally requested when pulling the named container.
    pub(crate) async fn get_image(&self, name: &ComponentName) -> Result<v1::Image> {
        let component_path = component_path(&name);

        let container_metadata = metadata(&component_path.join(CONTAINER_FILENAME))
            .await
            .context("Failed to get metadata for container file")?;
        let mut image_spec_file = File::open(&component_path.join(IMAGE_SPEC_FILENAME))
            .await
            .context("Failed to open image spec file")?;

        let mut serialized_image_spec = Vec::new();
        image_spec_file
            .read_to_end(&mut serialized_image_spec)
            .await
            .context("Failed reading image spec from file")?;
        let image_spec = v1::ImageSpec::decode(serialized_image_spec.as_slice())
            .context("Failed to decode image spec from file")?;

        Ok(v1::Image {
            id: name.to_string(),
            repo_tags: Vec::default(),
            repo_digests: Vec::default(),
            size: container_metadata.len() as u64,
            uid: None,
            username: String::default(),
            spec: Some(image_spec),
            pinned: false,
        })
    }
}

/// Assets for e.g. `00000000000000000000000000000001:bar.baz.Foo@1.0.0`
/// would be stored under `<STORE_ROOT>/00000000000000000000000000000001/bar.baz.Foo/1.0.0/`.
fn component_path(name: &ComponentName) -> PathBuf {
    Path::new(STORE_ROOT)
        .join(name.service.domain.to_string())
        .join(&name.service.service)
        .join(&name.version)
}

/// The container client fetches and processes blobs from a
/// [container registry](https://specs.opencontainers.org/distribution-spec/).
#[derive(Clone)]
struct ContainerClient {
    /// Basic HTTP client.
    http: Client,

    /// Set of registries that should be fetched via HTTP rather than HTTPS.
    insecure_registries: Arc<HashSet<String>>,

    /// Global Wasm engine to run hosted services.
    /// This must be the exact same engine used in the [store](ContainerStore).
    wasmtime: WasmEngine,
}

const MANIFEST_MIME: &str = "application/vnd.oci.image.manifest.v1+json";

impl ContainerClient {
    fn new(insecure_registries: HashSet<String>, wasmtime: &WasmEngine) -> Self {
        Self {
            http: Client::new(),
            insecure_registries: Arc::new(insecure_registries),
            wasmtime: wasmtime.clone(),
        }
    }

    async fn fetch(&self, registry: &str, name: &ComponentName) -> Result<Arc<Container>> {
        log_info!(component: name, "Fetching image from {:?}", registry);

        // Any URL path for `1234567890abcdef1234567890abcdef:package.Service`
        // would begin with `/v2/1234567890abcdef1234567890abcdef/071636b6167656e235562767963656/`.
        let service_url = format!(
            "{}://{}/v2/{}/{}",
            if self.insecure_registries.contains(registry) {
                "http"
            } else {
                "https"
            },
            registry,
            name.service.domain,
            // Repository namespace components must contain only lowercase letters and digits,
            // so hex-encode the service name.
            hexify_string(&name.service.service),
        );

        // Pull the manifest:
        // https://specs.opencontainers.org/distribution-spec/#pulling-manifests.
        let manifest_url = format!("{service_url}/manifests/{}", name.version);
        let response = self
            .http
            .get(&manifest_url)
            .header(ACCEPT, MANIFEST_MIME)
            .send()
            .await
            .with_context(|| format!("Failed fetching manifest: {:?}", manifest_url))?;
        if response.status() == HttpStatusCode::OK {
            let manifest = response
                .json::<ImageManifest>()
                .await
                .with_context(|| format!("Failed decoding manifest: {:?}", manifest_url))?;

            // All images consist of 2 layers:
            // the component byte code, followed by the serialized metadata.
            if manifest.layers.len() == 2 {
                // Fetch the layers in parallel.
                let component_fetch = spawn(self.clone().fetch_component(format!(
                    "{service_url}/blobs/{}",
                    manifest.layers.get(0).unwrap().digest,
                )));
                let metadata_result = self
                    .fetch_metadata(format!(
                        "{service_url}/blobs/{}",
                        manifest.layers.get(1).unwrap().digest,
                    ))
                    .await;

                // Propagate compilation errors first, then metadata parsing errors.
                let component = component_fetch
                    .await
                    .context("Failure joining fetch-component background task")??;
                let metadata = metadata_result?;

                log_info!(component: name, "Successful image fetch");
                Ok(Arc::new(Container {
                    component,
                    metadata,
                }))
            } else {
                Err(anyhow!(
                    "Unexpected container layer count: {:?}",
                    manifest.layers.len()
                ))
                .context(format!("Failed fetching manifest: {:?}", manifest_url))
            }
        } else {
            Err(anyhow!("Got HTTP {}", response.status().as_u16()))
                .context(format!("Failed fetching manifest: {:?}", manifest_url))
        }
    }

    async fn fetch_component(self, url: String) -> Result<Component> {
        Component::new(
            &self.wasmtime,
            self.fetch_blob(&url)
                .await
                .with_context(|| format!("Failure fetching component: {:?}", url))?,
        )
        .context("Component compilation error")
    }

    async fn fetch_metadata(&self, url: String) -> Result<Metadata> {
        // TODO: We're decoding this only to encode it again later.
        //       Avoid the unnecessary work.
        Metadata::decode(
            self.fetch_blob(&url)
                .await
                .with_context(|| format!("Failure fetching metadata: {:?}", url))?,
        )
        .context("Failure decoding metadata")
    }

    async fn fetch_blob(&self, url: &str) -> Result<Bytes> {
        let response = self.http.get(url).send().await.context(
            // Fails if there was an error while sending request,
            // redirect loop was detected or redirect limit was exhausted.
            "Error fetching blob",
        )?;
        if response.status() == HttpStatusCode::OK {
            response.bytes().await.context(
                // Not sure when this would ever happen.
                "Failed reading response",
            )
        } else {
            Err(anyhow!("Got HTTP {}", response.status().as_u16()))
        }
    }
}

/// See [spec](https://specs.opencontainers.org/image-spec/manifest/#image-manifest).
#[allow(dead_code)]
#[allow(non_snake_case)]
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
#[allow(dead_code)]
#[allow(non_snake_case)]
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
