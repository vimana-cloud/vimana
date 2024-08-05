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
use wasmtime::{Config as WasmConfig, Engine as WasmEngine, Store};
use wasmtime::component::Instance;

// TODO: This should be the protobuf of method configs
//       attached to the image.
//       Contains encoder, decoder, Wasm function name, etc.
type MethodConfig = u32;

/// A service implementation,
/// corresponding to a specific version of a service.
pub struct Implementation {

    // A running instance of the component.
    instance: Instance,

    // The store associated with `instance`.
    store: Mutex<Store<()>>,

    // A mapping from gRPC method names to method configs.
    methods: HashMap<String, MethodConfig>,
}

/// Use a two-level map structure to look up service implementations,
/// providing some degree of contention isolation between domains.
/// The top level keys are domains.
/// The lower level is [ComponentMap],
/// with keys composed of the service name and version.
/// Together, they are effectively a single key-value store
/// mapping (domain, service-name, version) keys to [Implementation] values.
type DomainMap = RwLock<HashMap<String, ComponentMap>>;

/// The lower level of [DomainMap].
/// Keys are of the form: <service-name> "@" <version>
type ComponentMap = RwLock<HashMap<String, InstancePool>>;

/// A pool of instances of the same component.
// TODO: Implement a real type.
type InstancePool = ();

/// Global runtime state for an Actio Work Node.
pub struct WorkRuntime {
    /// Local cache of [Implementation] information.
    implementations: DomainMap,
    /// Global Wasm engine to run hosted services.
    /// This is a cheap, thread-safe handle to the "real" engine.
    wasmtime: WasmEngine,
}

impl WorkRuntime {

    /// Return a new, empty [WorkRuntime].
    pub fn new() -> Self {
        WorkRuntime {
            implementations:
                RwLock::new(HashMap::new()),
            wasmtime:
                WasmEngine::new(
                    WasmConfig::new()
                        // Allows host functions to be `async` Rust.
                        // Means you have to use `Func::call_async` instead of `Func::call`.
                        // Components themselves effectively have a GIL
                        // because they need [exclusive access] to a Store.
                        .async_support(true)
                        // Epoch interruption for preemptive multithreading.
                        // https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html#method.epoch_interruption
                        .epoch_interruption(true)
                        // Enable support for various Wasm proposals...
                        .wasm_component_model(true)
                        .wasm_gc(true)
                        .wasm_tail_call(true)
                        .wasm_function_references(true)
                ).unwrap(),
        }
    }

    /// Get a loaded implementation.
    pub async fn get_instance_pool(&self, domain: &str, component_name: &str) -> Result<InstancePool, String> {
        // Look for an existing instance pool.
        let domain_map = self.implementations.read().await;
        match domain_map.get(domain) {
            Some(locked_component_map) => {
                let component_map = locked_component_map.read().await;
                match component_map.get(component_name) {
                    Some(pool) => Ok(pool.clone()),
                    None => {
                        // No existing instance pool; free all acquired read locks before fetching.
                        drop(component_map);
                        drop(domain_map);
                        self.new_instance_pool(domain, component_name).await
                    }
                }
            },
            None => {
                // No existing instance pool; free all acquired read locks before fetching.
                drop(domain_map);
                self.new_instance_pool(domain, component_name).await
            },
        }
    }

    /// Create a new instance pool for the named component.
    async fn new_instance_pool(&self, _domain: &str, _component_name: &str) -> Result<InstancePool, String> {
        todo!()
    }
}
