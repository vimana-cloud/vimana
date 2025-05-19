//! Client used to fetch and compile containers from a registry,
//! caching compiled components and container metadata locally.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use moka::future::Cache;
use prost::Message;
use reqwest::header::ACCEPT;
use reqwest::{Client, StatusCode as HttpStatusCode};
use serde::Deserialize;
use tokio::task::spawn;
use wasmtime::component::Component;
use wasmtime::Engine as WasmEngine;

use error::{log_error_status, log_info, Result};
use metadata_proto::work::runtime::Metadata;
use names::ComponentName;

/// Client used to fetch and compile containers from a registry,
/// caching compiled components and parsed container metadata locally.
#[derive(Clone)]
pub struct ContainerStore {
    /// Local in-memory cache of compiled/parsed containers.
    cache: Cache<ComponentName, Arc<Container>>,

    /// Means to fetch containers from a remote container registry.
    client: ContainerClient,
}

/// Ready-to-link container.
pub struct Container {
    /// Compiled component implementation.
    pub component: Component,

    /// Parsed container metadata.
    pub metadata: Metadata,

    /// Size, in kibibytes, of the serialized container blobs,
    /// which **approximates** the memory footprint of the cached container.
    ///
    /// 32 bits because that's what the Moka cache expects for entry weights.
    /// Not measured in bytes because that would make the maximum size 4 GiB.
    /// Using kibibytes instead gives us a max of 4TiB.
    kibibytes: u32,
}

impl ContainerStore {
    /// Return a new [store](ContainerStore)
    /// that fetches Vimana container images (composed of a component and associated metadata)
    /// from the given registry URL.
    /// Components will be instantiated using the provided [`Engine`](WasmEngine).
    /// Containers are cached locally in-memory
    /// up to the limit specified by `max_cache_kibibytes`.
    pub fn new(registry: String, max_cache_kibibytes: u64, wasmtime: &WasmEngine) -> Self {
        Self {
            cache: Cache::builder()
                .weigher(|_, container: &Arc<Container>| container.kibibytes)
                .max_capacity(max_cache_kibibytes)
                .build(),
            client: ContainerClient::new(registry, wasmtime),
        }
    }

    /// Return a compiled component implementation and its metadata,
    /// either copied from the local cache (if available),
    /// or fetched from a remote repository (and used to update the local cache).
    pub async fn get(&self, name: &ComponentName) -> Result<Arc<Container>> {
        self.cache
            .try_get_with_by_ref(name, self.client.fetch(name))
            .await
            .map_err(|status| status.as_ref().clone())
    }

    /// Attempt to populate the cache with the container for the given component
    /// in a background thread.
    /// A subsequent call to `get`, if successful, should finish more quickly.
    pub fn prefetch(&self, name: &ComponentName) {
        let store = self.clone();
        let name = name.clone();
        spawn(async move { store.get(&name).await });
    }
}

/// The container client fetches and processes blobs from a
/// [container registry](https://specs.opencontainers.org/distribution-spec/).
#[derive(Clone)]
struct ContainerClient(Arc<ContainerClientInner>);

/// Reference-counted to make [`ContainerClient`] cheaply cloneable
/// for parallel downloads.
struct ContainerClientInner {
    /// Basic HTTP client.
    http: Client,

    /// Scheme, host name, and optional port of the registry (e.g. `http://localhost:5000`).
    registry: String,

    /// Global Wasm engine to run hosted services.
    wasmtime: WasmEngine,
}

const MANIFEST_MIME: &str = "application/vnd.oci.image.manifest.v1+json";

impl ContainerClient {
    fn new(registry: String, wasmtime: &WasmEngine) -> Self {
        Self(Arc::new(ContainerClientInner {
            http: Client::new(),
            registry,
            wasmtime: wasmtime.clone(),
        }))
    }

    async fn fetch(&self, name: &ComponentName) -> Result<Arc<Container>> {
        log_info!("fetching-container", name, ());

        // Any URL path for `1234567890abcdef1234567890abcdef:package.Service`
        // would begin with `/v2/1234567890abcdef1234567890abcdef/071636b6167656e235562767963656/`.
        let service_url = format!(
            "{}/v2/{}/{}",
            self.0.registry,
            name.service.domain,
            // Repository namespace components must contain only lowercase letters and digits,
            // so hex-encode the service name.
            hexify(&name.service.service),
        );
        // Pull the manifest:
        // https://specs.opencontainers.org/distribution-spec/#pulling-manifests.
        let manifest_url = format!("{service_url}/manifests/{}", name.version);
        let response = self
            .0
            .http
            .get(&manifest_url)
            .header(ACCEPT, MANIFEST_MIME)
            .send()
            .await
            .map_err(
                // Fails if there was an error while sending request,
                // redirect loop was detected or redirect limit was exhausted.
                log_error_status!("get-manifest", name),
            )?;
        if response.status() == HttpStatusCode::OK {
            let manifest = response.json::<ImageManifest>().await.map_err(
                // JSON decoding failed.
                log_error_status!("decode-manifest", name),
            )?;

            // All images consist of 2 layers:
            // the component byte code, followed by the serialized metadata.
            if manifest.layers.len() == 2 {
                // Fetch the layers in parallel.
                let component_fetch = spawn(self.clone().fetch_component(
                    format!(
                        "{service_url}/blobs/{}",
                        manifest.layers.get(0).unwrap().digest,
                    ),
                    name.clone(),
                ));
                let metadata_result = self
                    .fetch_metadata(
                        format!(
                            "{service_url}/blobs/{}",
                            manifest.layers.get(1).unwrap().digest,
                        ),
                        &name,
                    )
                    .await;

                // Propagate compilation errors first, then metadata parsing errors.
                let (component, component_size) = component_fetch.await.map_err(
                    // Background task join error.
                    log_error_status!("fetch-component-join", name),
                )??;
                let (metadata, metadata_size) = metadata_result?;

                // Total size (in bytes) of the container (serialized).
                let total_size = usize::saturating_add(component_size, metadata_size);
                // Round up converting to kibibytes.
                let kibibytes = ((total_size as f64) / 1024.0).ceil() as u32;

                log_info!("fetched-container-success", name, ());
                Ok(Arc::new(Container {
                    component,
                    metadata,
                    kibibytes,
                }))
            } else {
                Err(log_error_status!("unexpected-container-layers", name)(
                    manifest.layers.len(),
                ))
            }
        } else if response.status() == HttpStatusCode::NOT_FOUND {
            Err(log_error_status!("manifest-not-found", name)(manifest_url))
        } else {
            Err(log_error_status!("get-manifest-status", name)(format!(
                "(status={} url={})",
                response.status().as_u16(),
                manifest_url
            )))
        }
    }

    async fn fetch_component(self, url: String, name: ComponentName) -> Result<(Component, usize)> {
        let byte_code = self.fetch_blob(url, &name).await?;
        let size = byte_code.len();
        Ok((
            Component::new(&self.0.wasmtime, byte_code).map_err(
                // Compilation error.
                log_error_status!("compile-component", &name),
            )?,
            size,
        ))
    }

    async fn fetch_metadata(&self, url: String, name: &ComponentName) -> Result<(Metadata, usize)> {
        let serialized = self.fetch_blob(url, name).await?;
        let size = serialized.len();
        Ok((
            Metadata::decode(serialized).map_err(
                // Malformed metadata.
                log_error_status!("decode-metadata", name),
            )?,
            size,
        ))
    }

    async fn fetch_blob(&self, url: String, name: &ComponentName) -> Result<Bytes> {
        let response = self.0.http.get(&url).send().await.map_err(
            // Fails if there was an error while sending request,
            // redirect loop was detected or redirect limit was exhausted.
            log_error_status!("get-blob", name),
        )?;
        if response.status() == HttpStatusCode::OK {
            response.bytes().await.map_err(
                // Not sure when this would ever happen.
                log_error_status!("blob-bytes", name),
            )
        } else if response.status() == HttpStatusCode::NOT_FOUND {
            Err(log_error_status!("blob-not-found", name)(url))
        } else {
            // Catch-all non-OK status code.
            Err(log_error_status!("get-blob-status", name)(format!(
                "(status={} url={})",
                response.status().as_u16(),
                url
            )))
        }
    }
}

const HEX_CHARS: &[u8] = b"0123456789abcdef";

/// Hex-encode a string,
/// returning a new string with equivalient data and double the length,
/// using only the characters `[0-9a-f]`,
/// nibble-wise little-endian (lower nibble comes first).
fn hexify(string: &str) -> String {
    let mut v = Vec::with_capacity(string.len() * 2);

    for &byte in string.as_bytes().iter() {
        v.push(HEX_CHARS[(byte & 0xf) as usize]);
        v.push(HEX_CHARS[(byte >> 4) as usize]);
    }

    unsafe { String::from_utf8_unchecked(v) }
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
