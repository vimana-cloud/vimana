//! Client used to fetch and compile containers from a registry,
//! caching compiled components and container metadata locally.

use std::collections::{HashMap, HashSet};
use std::fs::{
    create_dir_all as sync_create_dir_all, metadata as sync_metadata,
    remove_dir as sync_remove_dir, remove_file as sync_remove_file, File as SyncFile,
};
use std::io::{Read, Write};
use std::mem::{drop, size_of};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as SyncMutex;

use anyhow::{anyhow, Context, Error, Result};
use api_proto::runtime::v1;
use bytes::Bytes;
use prost::Message;
use reqwest::header::ACCEPT;
use reqwest::{Client, StatusCode as HttpStatusCode};
use serde::Deserialize;
use tokio::task::{spawn, spawn_blocking};
use wasmtime::component::Component;
use wasmtime::Engine as WasmEngine;

use logging::log_info;
use metadata_proto::work::runtime::Metadata;
use names::ComponentName;

/// Each component directory under [store root](ContainerStore::root)
/// has a file called `container` containing the pre-compiled [Component] and the [Metadata].
const CONTAINER_FILENAME: &str = "container";

/// Each image directory under [store root](ContainerStore::root)
/// also has a file called `image-spec.binpb` containing the [image spec](v1::ImageSpec)
/// that was originally specified when pulling the image.
const IMAGE_SPEC_FILENAME: &str = "image-spec.binpb";

/// Client used to fetch and compile containers from a registry,
/// caching compiled components and parsed container metadata locally.
#[derive(Clone)]
pub(crate) struct ContainerStore {
    /// Root of the filesystem tree where images are save locally on pull.
    /// See [`component_path`](Self::component_path).
    root: PathBuf,

    /// The total number of bytes and inodes used to store images locally.
    filesystem_usage: Arc<SyncMutex<FilesystemUsage>>,

    /// Means to fetch containers from a remote container registry.
    client: ContainerClient,

    /// Global Wasm engine to run hosted servers.
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

/// Information about filesystem usage for this Vimana image store.
#[derive(Clone)]
pub(crate) struct FilesystemUsage {
    /// Total number of bytes from every file under the [root](ContainerStore::root).
    pub(crate) bytes: u64,

    /// Total number of files and directories under the [root](ContainerStore::root).
    pub(crate) inodes: u64,
}

impl ContainerStore {
    /// Return a new [store](ContainerStore).
    /// Components will be instantiated using the provided [`Engine`](WasmEngine).
    pub(crate) fn new(
        root: &str,
        insecure_registries: HashSet<String>,
        wasmtime: &WasmEngine,
    ) -> Result<Self> {
        // The image filesystem root path reported by `ImageFsInfo` to Kubelet must exist,
        // otherwise Kubelet will get confused and evict all the pods,
        // including the system pods managed by the OCI runtime,
        // so make sure it exists at the start of the container store's life.
        sync_create_dir_all(root)
            .with_context(|| format!("Failed to create image root directory: {:?}", root))?;

        Ok(Self {
            root: PathBuf::from(&root),
            filesystem_usage: Arc::new(SyncMutex::new(FilesystemUsage {
                bytes: 0,
                inodes: 0,
            })),
            client: ContainerClient::new(insecure_registries, wasmtime),
            wasmtime: wasmtime.clone(),
        })
    }

    /// Return the path to the root directory where images are stored on pull.
    pub(crate) fn mountpoint(&self) -> String {
        String::from(self.root.to_string_lossy())
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
        // TODO: Prefer to use wasmtime's `Engine::precompile_component`.
        let serialized_component = container.component.serialize()?;
        let serialized_metadata = container.metadata.encode_to_vec();
        let serialized_image_spec = image_spec.encode_to_vec();

        let component_path = self.component_path(name);
        let container_path = component_path.join(CONTAINER_FILENAME);
        let image_spec_path = component_path.join(IMAGE_SPEC_FILENAME);
        let filesystem_usage = self.filesystem_usage.clone();

        let result = spawn_blocking(move || {
            // This locks the mutex so you can't remove or pull anything else until it's finished.
            let mut filesystem_usage = filesystem_usage
                .lock()
                .map_err(|_| anyhow!("Filesystem usage lock poisoned"))?;

            // Determine how many new inodes will be created by this process.
            let (dir_inodes, container_inode, image_spec_inode) = if component_path.exists() {
                // If for some reason the files already existed, don't count them as new inodes.
                (
                    0,
                    (!container_path.exists()) as u64,
                    (!image_spec_path.exists()) as u64,
                )
            } else {
                let server_path = component_path.parent().unwrap();
                if server_path.exists() {
                    (1, 1, 1)
                } else {
                    let domain_path = server_path.parent().unwrap();
                    if domain_path.exists() {
                        (2, 1, 1)
                    } else {
                        (3, 1, 1)
                    }
                }
            };

            sync_create_dir_all(component_path.as_path()).with_context(|| {
                format!("Failed to create image directory: {:?}", component_path)
            })?;
            filesystem_usage.inodes += dir_inodes;
            let mut container_file =
                SyncFile::create(container_path.as_path()).with_context(|| {
                    format!("Failed to create container file: {:?}", container_path)
                })?;
            filesystem_usage.inodes += container_inode;
            let mut image_spec_file =
                SyncFile::create(image_spec_path.as_path()).with_context(|| {
                    format!("Failed to create image spec file: {:?}", image_spec_path)
                })?;
            filesystem_usage.inodes += image_spec_inode;

            // Write the length of the component to the file first (as a little-endian `u64`),
            // then the component's pre-compiled bytes.
            let module_length = serialized_component.len() as u64;
            container_file
                .write_all(&module_length.to_le_bytes())
                .context("Failed writing length to container file")?;
            filesystem_usage.bytes += size_of::<u64>() as u64;
            container_file
                .write_all(&serialized_component)
                .context("Failed writing component to container file")?;
            filesystem_usage.bytes += module_length;
            // Free space as soon as possible since it could be big.
            drop(serialized_component);

            container_file
                .write_all(&serialized_metadata)
                .context("Failed writing metadata to container file")?;
            filesystem_usage.bytes += serialized_metadata.len() as u64;
            drop(serialized_metadata);

            image_spec_file
                .write_all(&serialized_image_spec)
                .context("Failed writing to image spec file")?;
            filesystem_usage.bytes += serialized_image_spec.len() as u64;

            // Make sure the files are physically written to disk.
            container_file
                .sync_all()
                .context("Failed syncing container file")?;
            image_spec_file
                .sync_all()
                .context("Failed syncing image spec file")?;

            Ok(())
        })
        .await
        .context("Failed joining blocking thread to pull image")?;

        if result.is_ok() {
            log_info!(component: name, "Successful image pull");
        }

        result
    }

    /// Return a compiled component implementation and its metadata.
    pub(crate) async fn get(&self, name: &ComponentName) -> Result<Container> {
        let component_path = self.component_path(name);
        let container_path = component_path.join(CONTAINER_FILENAME);

        let (serialized_component, serialized_metadata) = spawn_blocking(move || {
            let mut container_file = SyncFile::open(container_path.as_path())
                .with_context(|| format!("Failed to open container file: {:?}", container_path))?;

            let mut component_size_bytes = [0; size_of::<u64>()];
            container_file
                .read_exact(&mut component_size_bytes)
                .context("Failed reading length from container file")?;
            let component_size = u64::from_le_bytes(component_size_bytes);
            let mut serialized_component = Vec::with_capacity(component_size as usize);
            unsafe { serialized_component.set_len(component_size as usize) };
            container_file
                .read_exact(&mut serialized_component)
                .context("Failed reading component from container file")?;

            let mut serialized_metadata = Vec::new();
            container_file
                .read_to_end(&mut serialized_metadata)
                .context("Failed reading metadata from container file")?;

            Ok::<_, Error>((serialized_component, serialized_metadata))
        })
        .await
        .context("Failed joining blocking thread to read image")??;

        let component = unsafe { Component::deserialize(&self.wasmtime, &serialized_component) }
            .with_context(|| {
                format!(
                    "Failed deserializing component (length = {})",
                    serialized_component.len(),
                )
            })?;
        // Free space as soon as possible since it could be big.
        drop(serialized_component);

        let metadata = Metadata::decode(serialized_metadata.as_slice())
            .context("Failed to decode metadata from container file")?;

        Ok(Container {
            component,
            metadata,
        })
    }

    /// Return metadata about the image originally requested when pulling the named container.
    pub(crate) async fn get_image(&self, name: &ComponentName) -> Result<v1::Image> {
        let component_path = self.component_path(name);
        let container_path = component_path.join(CONTAINER_FILENAME);
        let image_spec_path = component_path.join(IMAGE_SPEC_FILENAME);

        let (container_size, serialized_image_spec) = spawn_blocking(move || {
            let container_metadata =
                sync_metadata(container_path.as_path()).with_context(|| {
                    format!(
                        "Failed to get metadata for container file: {:?}",
                        container_path
                    )
                })?;
            let mut image_spec_file =
                SyncFile::open(image_spec_path.as_path()).with_context(|| {
                    format!("Failed to open image spec file: {:?}", image_spec_path)
                })?;

            let mut serialized_image_spec = Vec::new();
            image_spec_file
                .read_to_end(&mut serialized_image_spec)
                .context("Failed reading image spec from file")?;

            Ok::<_, Error>((container_metadata.len(), serialized_image_spec))
        })
        .await
        .context("Failed joining blocking thread to read image metadata")??;

        let image_spec = v1::ImageSpec::decode(serialized_image_spec.as_slice())
            .context("Failed to decode image spec from file")?;

        Ok(v1::Image {
            id: name.to_string(),
            repo_tags: Vec::default(),
            repo_digests: Vec::default(),
            size: container_size,
            uid: None,
            username: String::default(),
            spec: Some(image_spec),
            pinned: false,
        })
    }

    /// Delete an image that has been pulled and saved locally.
    pub(crate) async fn remove(&self, name: &ComponentName) -> Result<()> {
        let component_path = self.component_path(name);
        let container_path = component_path.join(CONTAINER_FILENAME);
        let image_spec_path = component_path.join(IMAGE_SPEC_FILENAME);
        let filesystem_usage = self.filesystem_usage.clone();

        spawn_blocking(move || {
            // This locks the mutex so you can't remove or pull anything else until it's finished.
            let mut filesystem_usage = filesystem_usage
                .lock()
                .map_err(|_| anyhow!("Filesystem usage lock poisoned"))?;

            // Read file metadata so we know how many bytes we're freeing up.
            let container_metadata =
                sync_metadata(container_path.as_path()).with_context(|| {
                    format!(
                        "Failed to get metadata for container file: {:?}",
                        container_path
                    )
                })?;
            sync_remove_file(container_path.as_path())
                .with_context(|| format!("Failed removing container file: {:?}", container_path))?;
            filesystem_usage.bytes -= container_metadata.len();
            filesystem_usage.inodes -= 1;

            let image_spec_metadata =
                sync_metadata(image_spec_path.as_path()).with_context(|| {
                    format!(
                        "Failed to get metadata for image spec file: {:?}",
                        image_spec_path
                    )
                })?;
            sync_remove_file(image_spec_path.as_path()).with_context(|| {
                format!("Failed removing image spec file: {:?}", image_spec_path)
            })?;
            filesystem_usage.bytes -= image_spec_metadata.len();
            filesystem_usage.inodes -= 1;

            // Remove the version, server, and domain directories if they're empty.
            for directory in component_path.ancestors().take(3) {
                if sync_remove_dir(directory).is_ok() {
                    filesystem_usage.inodes -= 1;
                } else {
                    break;
                }
            }

            Ok(())
        })
        .await
        .context("Failed joining blocking thread to remove image")?
    }

    /// Return the total number of inodes and bytes used to store images locally.
    pub(crate) async fn filesystem_usage(&self) -> Result<FilesystemUsage> {
        let filesystem_usage = self.filesystem_usage.clone();
        spawn_blocking(move || {
            // This locks the mutex so you can't remove or pull anything else until it's finished.
            let filesystem_usage = filesystem_usage
                .lock()
                .map_err(|_| anyhow!("Filesystem usage lock poisoned"))?;

            Ok(filesystem_usage.clone())
        })
        .await
        .context("Failed joining blocking thread to get filesystem usage")?
    }

    /// Assets for e.g. `00000000000000000000000000000001:bar.baz.Foo@1.0.0`
    /// would be stored under `<root>/00000000000000000000000000000001/bar.baz.Foo/1.0.0/`.
    fn component_path(&self, name: &ComponentName) -> PathBuf {
        self.root
            .join(name.server.domain.to_string())
            .join(&name.server.server)
            .join(&name.version)
    }
}

/// The container client fetches and processes blobs from a
/// [container registry](https://specs.opencontainers.org/distribution-spec/).
#[derive(Clone)]
struct ContainerClient {
    /// Basic HTTP client.
    http: Client,

    /// Set of registries that should be fetched via HTTP rather than HTTPS.
    insecure_registries: Arc<HashSet<String>>,

    /// Global Wasm engine to run hosted servers.
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

        // Any URL path for `1234567890abcdef1234567890abcdef:server-id`
        // would begin with `/v2/1234567890abcdef1234567890abcdef/server-id/`.
        let server_url = format!(
            "{}://{}/v2/{}/{}",
            if self.insecure_registries.contains(registry) {
                "http"
            } else {
                "https"
            },
            registry,
            name.server.domain,
            name.server.server,
        );

        // Pull the manifest:
        // https://specs.opencontainers.org/distribution-spec/#pulling-manifests.
        let manifest_url = format!("{server_url}/manifests/{}", name.version);
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
                    "{server_url}/blobs/{}",
                    manifest.layers.get(0).unwrap().digest,
                )));
                let metadata_result = self
                    .fetch_metadata(format!(
                        "{server_url}/blobs/{}",
                        manifest.layers.get(1).unwrap().digest,
                    ))
                    .await;

                // Propagate compilation errors first, then metadata parsing errors.
                let component = component_fetch
                    .await
                    .context("Failure joining fetch-component background task")??;
                let metadata = metadata_result?;

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
