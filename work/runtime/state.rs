//! Data structures to manage work runtime pods.
//!
//! Each work node runs a single instance of the runtime,
//! which governs the node by serving gRPC services on two ports:
//!
//! - UDP 443 (HTTPS/3)
//!   fields requests from Ingress to all hosted services.
//! - Unix `/run/vimana/workd.sock`
//!   handles orchestration requests from Kubelet.
#![feature(async_closure)]

use std::collections::HashMap;
use std::error::Error as StdError;

use papaya::HashMap as LockFreeConcurrentHashMap;
use tokio::sync::Mutex as AsyncMutex;
use tonic::transport::channel::Channel;
use wasmtime::component::{InstancePre, Linker};
use wasmtime::{Config as WasmConfig, Engine as WasmEngine, Store};

use api_proto::runtime::v1::image_service_client::ImageServiceClient;
use api_proto::runtime::v1::runtime_service_client::RuntimeServiceClient;
use container_proto::work::runtime::container::Container;
use containers::ContainerStore;
use error::{Error, Result};
use names::{ComponentName, PodId, PodName};

/// Global runtime state for a work node.
pub struct WorkRuntime {
    /// Global Wasm engine to run hosted services.
    /// This is a cheap, thread-safe handle to the "real" engine.
    wasmtime: WasmEngine,

    /// Map of locally running components,
    /// each representing a K8s pod with a single container.
    /// Lock-freedom is important to help isolate tenants from one another.
    pods: LockFreeConcurrentHashMap<ComponentName, PodPool>,

    /// Remote store from which to retrieve container images by ID,
    /// which can then be loaded into pods.
    containers: ContainerStore,

    /// Client to a downstream OCI container runtime (e.g. containerd or cri-o)
    /// so work nodes can run traditional OCI containers as well.
    pub oci_runtime: AsyncMutex<RuntimeServiceClient<Channel>>,

    /// Client to the downstream OCI CRI image service.
    pub oci_image: AsyncMutex<ImageServiceClient<Channel>>,
}

/// A group of pods which are all running the same container (*a.k.a.* component).
struct PodPool {
    // TODO: This is a very naive implementation for initial POC. Make it better.
    /// Map from pod ID to pod instance.
    pods: AsyncMutex<HashMap<String, Pod>>,
}

/// A pod *roughly* corresponds to a component "instance".
///
/// Technically, rather than an instance,
/// it corresponds to an [`InstancePre`],
/// which can be used to efficiently instantiate new instances on the fly.
///
/// A new instance is created to handle each request.
/// This is the only means of multi-threaded execution in wasmtime until
/// [shared-everything threads](https://github.com/WebAssembly/shared-everything-threads/)
/// (or something similar) becomes available.
/// [See here](https://bytecodealliance.zulipchat.com/#narrow/stream/217126-wasmtime/topic/Concurrent.20execution).
///
/// Currently, each pod can have
/// [shared memories](https://docs.rs/wasmtime/latest/wasmtime/struct.SharedMemory.html)
/// which are shared among all instances.
pub struct Pod {
    // An efficient means of instantiating new instances.
    instantiator: InstancePre<HostState>,
}

/// State available to host-defined functions.
type HostState = ();

impl WorkRuntime {
    /// Return a new runtime with no running pods.
    pub async fn new(
        oci_runtime: RuntimeServiceClient<Channel>,
        oci_image: ImageServiceClient<Channel>,
    ) -> Result<Self> {
        Ok(Self {
            wasmtime: Self::default_engine()?,
            pods: LockFreeConcurrentHashMap::new(),
            containers: ContainerStore::new(),
            oci_runtime: AsyncMutex::new(oci_runtime),
            oci_image: AsyncMutex::new(oci_image),
        })
    }

    // A new instance of the default engine for this runtime.
    fn default_engine() -> Result<WasmEngine> {
        WasmEngine::new(
            WasmConfig::new()
                // Allow host functions to be `async` Rust.
                // Means you have to use `Func::call_async` instead of `Func::call`.
                .async_support(true)
                // Epoch interruption for preemptive multithreading.
                // https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html#method.epoch_interruption
                .epoch_interruption(true)
                // Enable support for various Wasm proposals...
                .wasm_component_model(true)
                .wasm_gc(true)
                .wasm_tail_call(true)
                .wasm_function_references(true),
        )
        .map_err(|err| Error::wrap(ENGINE_ALLOCATION_ERROR, err))
    }

    /// Add a new, empty pod to the pool, in a reserved state.
    ///
    /// Reserved pods are unusable
    /// until a container is [created](Self::create_container) within them.
    pub async fn add_pod(&self, name: ComponentName) -> PodId {
        let pods = self.pods.pin();
        pods.get_or_insert_with(name, PodPool::new).add_pod()
    }

    /// Create a container in a reserved pod.
    pub async fn create_container(&self, pod: &PodName) -> Result<()> {
        let container = self.containers.get_container(&pod.component)?;

        let pods = self.pods.pin();
        match pods.get(&pod.component) {
            Some(pool) => pool.create_container(pod.pod, container),
            None => Err(Error::leaf(format!(
                "Cannot create a container without an existing pod: {pod}"
            ))),
        }
    }

    pub async fn invoke_rpc(
        &self,
        component: &ComponentName,
        rpc: &str,
        request: i32,  // TODO: This should be the request buffer.
        response: i32, // TODO: This should be the response buffer.
    ) -> Result<()> {
        let pods = self.pods.pin();
        match pods.get(component) {
            Some(pod) => {
                todo!()
            }
            None => Err(Error::leaf(format!(
                "No running pods for component: {}",
                component
            ))),
        }
    }
}

const ENGINE_ALLOCATION_ERROR: &str = "Error allocating engine";

impl PodPool {
    fn new() -> Self {
        Self {
            pods: AsyncMutex::new(HashMap::new()),
        }
    }

    fn add_pod(&self) -> PodId {
        // TODO: Something real.
        0usize
    }

    fn create_container(&self, pod_id: PodId, container: Container) -> Result<()> {
        // TODO: Something real.
        Ok(())
    }
}
