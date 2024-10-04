/// Work node runtime state and documentation.
///
/// Each work node runs a single instance of the runtime,
/// which governs the node by serving gRPC services on two ports:
///
/// - UDP 443 (HTTPS/3)
///   fields requests from Ingress to all hosted services.
/// - Unix `/run/actio/container-runtime-interface.sock`
///   handles orchestration requests from Kubelet.
use std::collections::HashMap;
use std::mem::drop;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use wasmtime::component::{InstancePre, Linker};
use wasmtime::{Config as WasmConfig, Engine as WasmEngine, Store};

use containers::ContainerStore;
use error::{Error, Result};
use names::FullVersionName;
use pods_proto::pods::PodConfig;
use pool::{Key, KeyedPodPool, Pod};

// TODO: There may be a better data structure for pod pools.
/// Use a two-level map structure to look up pods,
/// providing some degree of contention isolation between domains.
/// The top level keys are domains.
/// The lower level is [ComponentMap],
/// with keys composed of the service name and version.
/// Together, they are effectively a single key-value store
/// mapping (domain, service-name, version) keys to [Pod] values.
type DomainMap = RwLock<HashMap<String, ComponentMap>>;

/// The lower level of [DomainMap].
/// Keys are of the form: <service-name> "@" <version>
type ComponentMap = RwLock<HashMap<String, KeyedPodPool>>;

/// Global runtime state for a work node.
pub struct WorkRuntime {
    /// Global Wasm engine to run hosted services.
    /// This is a cheap, thread-safe handle to the "real" engine.
    wasmtime: WasmEngine,

    /// Local cache of [Pod] information.
    pods: DomainMap,

    /// Store from which to retrieve container images by ID.
    containers: ContainerStore,
}

impl WorkRuntime {
    /// Return a new, empty [WorkRuntime].
    pub fn new() -> Result<Self> {
        Ok(WorkRuntime {
            wasmtime: Self::default_engine()?,
            pods: RwLock::new(HashMap::new()),
            containers: ContainerStore::new(),
        })
    }

    // A new instance of the default engine for this runtime.
    pub fn default_engine() -> Result<WasmEngine> {
        WasmEngine::new(
            WasmConfig::new()
                // Allows host functions to be `async` Rust.
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
    pub async fn add_pod(&self, name: FullVersionName) -> Result<Key> {
        // Optimistically try read-locking first.
        let domain_map = self.pods.read().await;
        match domain_map.get(&name.domain) {
            Some(component_map) => self.add_pod_for_domain(component_map, name).await,
            None => {
                // No existing pods for the domain.
                // Drop the read-lock then get a write-lock for the whole pool.
                drop(domain_map);
                let mut domain_map_mut = self.pods.write().await;

                // There may have been a concurrent insertion
                // in between the first check and acquiring the write-lock.
                if let Some(concurrent_insertion) =
                    domain_map_mut.insert(name.domain.clone(), RwLock::new(HashMap::new()))
                {
                    // Defer to the prior insertion (insert it back).
                    domain_map_mut.insert(name.domain.clone(), concurrent_insertion);
                }
                drop(domain_map_mut); // Drop the write-lock.

                // Try again with a read-lock,
                // now that we're *pretty* sure the lookup will work.
                match self.pods.read().await.get(&name.domain) {
                    Some(component_map) => self.add_pod_for_domain(component_map, name).await,
                    None => Err(Error::leaf("Memory error while inserting pod into pool")),
                }
            }
        }
    }

    async fn add_pod_for_domain(
        &self,
        component_map: &ComponentMap,
        name: FullVersionName,
    ) -> Result<Key> {
        let component_name = name.without_domain();
        let mut component_map = component_map.write().await;
        match component_map.get_mut(&component_name) {
            Some(pool) => pool.add_pod(),
            None => {
                // TODO: Figure out how to get a real PodConfig in here.
                let config = PodConfig::default();
                let mut pool = KeyedPodPool::new(config);
                let key = pool.add_pod();
                component_map.insert(component_name, pool);
                key
            }
        }
    }

    /// Create a container in a reserved pod.
    pub async fn create_container(&self, key: Key, name: FullVersionName) -> Result<()> {
        let (config, pod) = self.containers.new_container(&name)?;
        let component_name = name.without_domain();

        if let Some(component_map) = self.pods.read().await.get(&name.domain) {
            if let Some(pool) = component_map.write().await.get_mut(&component_name) {
                return pool.create_container(key, pod);
            }
        }
        Err(Error::leaf(format!("Wrong pod for container: {key}")))
    }
}

const ENGINE_ALLOCATION_ERROR: &str = "Error allocating engine";
