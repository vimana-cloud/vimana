#![feature(async_fn_traits)]
#![feature(async_closure)] // test only
#![feature(noop_waker)] // test only

use std::collections::{HashMap, HashSet};
use std::ops::{AsyncFnOnce, DerefMut};
use std::sync::{Arc, Mutex as SyncMutex};

use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use wasmtime::component::{InstancePre, Linker};
use wasmtime::Engine as WasmEngine;

use error::{Error, Result};
use pods_proto::pods::PodConfig;

const RUN_QUEUE_SEMAPHORE_CLOSED_MSG: &str = "Run queue semaphore closed";
const PANICKED_HOLDING_GUARD_MSG: &str = "Another thread panicked while holding the objects guard";

/// State available to host-defined functions.
type HostState = ();

// TODO: Add shared memories.
/// A pod *roughly* corresponds to an implementation component "instance".
///
/// Technically, rather than an instance,
/// it corresponds to an [`InstancePre`] and [`Linker`],
/// which can be used to efficiently instantiate new instances on the fly.
/// A new instance is created to handle each request.
/// This is the only means of multi-threaded execution in wasmtime until
/// [shared-everything threads](https://github.com/WebAssembly/shared-everything-threads/)
/// (or similar) become available.
/// [See here](https://bytecodealliance.zulipchat.com/#narrow/stream/217126-wasmtime/topic/Concurrent.20execution).
///
/// Currently, each pod can have
/// [shared memories](https://docs.rs/wasmtime/latest/wasmtime/struct.SharedMemory.html)
/// which are shared among all instances.
///
/// If an implementation component uses two shared memories,
/// and a work node is running three pods for that component,
/// it will have six shared memories for it in total,
/// and will distribute incoming requests round-robin between pods.
pub struct Pod {
    // An efficient means of instantiating new instances.
    instantiator: InstancePre<HostState>,

    // Linker used to instantiate new instances.
    linker: Linker<HostState>,
}

/// A numeric key type.
pub type Key = usize;

/// A shared [`Pod`] pool
/// where each value is guarded by a mutex
/// and identified by a unique, auto-generated [key](Key).
pub struct KeyedPodPool {
    /// Information common to all pods in the pool.
    config: PodConfig,

    /// The collection of objects in the pool.
    /// The entire collection is guarded by a synchronous mutex,
    /// and each object is guarded by an `Arc` and asynchronous mutex.
    /// Operations on the collection must be quick,
    /// but operations on individual objects can be slow.
    objects: SyncMutex<StackMap>,

    /// Flag to fairly queue tasks executing [`run`](Self::run).
    /// The number of permits is never larger than the size of the pool.
    /// That is, you should never be able to acquire a permit
    /// unless an object in the pool is idle.
    run_queue: Semaphore,
}

/// A cross between a stack and a hash map
/// with auto-generated numeric [keys](Key).
struct StackMap {
    // TODO: Use a single key generator for the whole runtime, rather than per-stackmap (otherwise you'll re-use "unique" keys).
    /// The next available key that has never been reserved or used.
    /// Starts at 1 and increases monotonically.
    next_key: Key,

    /// Keys must be reserved before they can be used.
    /// Each key can only be reserved once, then used once.
    reserved: HashSet<Key>,

    /// Each value is owned by a shared mutex.
    /// Each key is cloned in the stack.
    map: HashMap<Key, Arc<AsyncMutex<Pod>>>,

    /// Contains keys from the map in some order.
    /// May contain some keys that are no longer in the map
    /// because they were deleted.
    stack: Vec<Key>,
}

impl KeyedPodPool {
    /// Return a new, empty pool.
    pub fn new(config: PodConfig) -> Self {
        Self {
            config,
            objects: SyncMutex::new(StackMap::new()),
            run_queue: Semaphore::new(0),
        }
    }

    /// Return the [config](PodConfig)
    /// that applies to all the pods of this pool.
    pub fn config(&self) -> &PodConfig {
        &self.config
    }

    /// Add a new, empty pod to the pool.
    /// Return the auto-generated key.
    pub fn add_pod(&mut self) -> Result<Key> {
        Ok(self
            .objects
            .lock()
            .map_err(|source| Error::leaf(String::from(PANICKED_HOLDING_GUARD_MSG)))?
            .deref_mut()
            .reserve()?)
    }

    /// Add a new, empty pod to the pool.
    /// Return the auto-generated key.
    pub fn create_container(&mut self, key: Key, pod: Pod) -> Result<()> {
        // Add the permit last, to protect the stack in StackMap::acquire_some.
        self.run_queue.add_permits(1);
        todo!()
    }

    /// Run the provided job (async function),
    /// passing it an (unlocked) object from the pool,
    /// and return the result.
    pub async fn run<F, T>(&self, job: F) -> Result<T>
    where
        F: AsyncFnOnce(Arc<AsyncMutex<Pod>>) -> T,
    {
        // Acquire a "run permit" to ensure fair access to the overall pool.
        // This permit must be held until running is complete.
        // Would only fail if the semaphore has been closed.
        let _run_permit = self
            .run_queue
            .acquire()
            .await
            .map_err(|source| Error::wrap(RUN_QUEUE_SEMAPHORE_CLOSED_MSG, source))?;

        // Should only fail if another thread panicked while holding the `objects` guard.
        let (key, pod): (Key, Arc<AsyncMutex<Pod>>) = self
            .objects
            .lock()
            .map_err(|source| Error::leaf(PANICKED_HOLDING_GUARD_MSG))?
            .deref_mut()
            .acquire_some()?;

        let result = job(pod).await;

        self.objects
            .lock()
            .map_err(|source| Error::leaf(PANICKED_HOLDING_GUARD_MSG))?
            .deref_mut()
            .replace(key);

        Ok(result)
    }

    /// The returned object may still be referenced by other task(s),
    /// but they should try to drop those references ASAP after freeing the mutex.
    pub async fn delete(&mut self, key: &Key) -> Result<Arc<AsyncMutex<Pod>>> {
        // First, remove a permit, to protect the stack in StackMap::acquire_some.
        self.run_queue
            .acquire()
            .await
            .map_err(|source| Error::wrap(RUN_QUEUE_SEMAPHORE_CLOSED_MSG, source))?
            .forget();

        let result = self
            .objects
            .lock()
            .map_err(|source| Error::leaf(PANICKED_HOLDING_GUARD_MSG))
            .and_then(|mut objects| objects.deref_mut().delete(key));

        // If deletion failed, add the run permit back,
        // so as not to deplete permits when the supplied `key` is not found.
        if result.is_err() {
            self.run_queue.add_permits(1);
        }

        result
    }
}

impl StackMap {
    /// Create a new stack-map with a single object inside.
    pub(crate) fn new() -> Self {
        StackMap {
            stack: Vec::new(),
            reserved: HashSet::new(),
            map: HashMap::new(),
            next_key: 1,
        }
    }

    /// Add a new pod to the stack-map.
    /// Return the auto-generated key.
    pub(crate) fn reserve(&mut self) -> Result<Key> {
        if self.map.len() + self.reserved.len() >= Semaphore::MAX_PERMITS {
            return Err(Error::leaf(format!(
                "Maximum pool size exceeded: {}",
                Semaphore::MAX_PERMITS
            )));
        }

        let key: Key = self.next_key;
        self.next_key += 1;

        let already_present: bool = self.reserved.insert(key);
        // Since the keys are being auto-generated,
        // we're *pretty* sure collisions are impossible,
        // but can't hurt to double-check in testing environments.
        debug_assert!(!already_present);

        Ok(key)
    }

    /// Return an "arbitrary" pod from the map, along with its key.
    /// The caller must explicitly [replace](Self::replace) the key,
    /// when finished with the pod, to make it available to other tasks;
    /// acquired exclusively.
    ///
    /// In practice,
    /// this is the pod most recently either [pushed](Self::push) or [replaced](Self::replace)
    /// which is not already acquired by another task.
    ///
    /// Values can be removed using [`Self::delete`].
    pub(crate) fn acquire_some(&mut self) -> Result<(Key, Arc<AsyncMutex<Pod>>)> {
        match self.stack.pop() {
            Some(key) => match self.map.get(&key) {
                Some(pod) => Ok((key, pod.clone())),

                // This happens
                // because `delete` removes an entry from the map without touching the stack.
                // Discard this key and take the next one.
                // The external semaphore should have been adjusted first,
                // so popping the stack again should return something.
                None => self.acquire_some(),
            },

            // This should be made impossible via the external semaphore.
            None => Err(Error::leaf("Semaphore corruption in pod poll")),
        }
    }

    /// Must be called when an [acquired](Self::acquire_some) object is no longer needed.
    pub(crate) fn replace(&mut self, key: Key) {
        self.stack.push(key)
    }

    pub(crate) fn delete(&mut self, key: &Key) -> Result<Arc<AsyncMutex<Pod>>> {
        self.map
            .remove(key)
            .ok_or_else(|| Error::leaf("Attempted to delete non-existent pod"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::ops::Deref;
    use std::pin::pin;
    use std::task::{Context, Waker};

    use tokio::sync::mpsc::{channel, Sender};
    use tokio::time::{sleep, Duration};
    use wasmtime::component::Component;
    use wasmtime::Engine as WasmEngine;

    use state::WorkRuntime;

    fn pod(engine: &WasmEngine) -> Pod {
        let linker = Linker::new(engine);
        let component = Component::new(engine, bytes).unwrap();
        Pod {
            instantiator: linker.instantiate_pre(&component).unwrap(),
            linker,
        }
    }

    #[tokio::test]
    async fn run_singleton() {
        let pool = KeyedPodPool::new(PodConfig::default());
        let pod = pod(&WorkRuntime::default_engine().unwrap());

        //let result = pool
        //    .run(async |object| {
        //        let actual_addr = object.lock().await.deref_mut().as_bytes().as_ptr();

        //        assert_eq!(actual_addr, expected_addr);

        //        "result"
        //    })
        //    .await;

        //assert_eq!(result.unwrap(), "result");
    }

    //#[tokio::test]
    //async fn run_multi() {
    //    let (tx_1, mut rx_1) = channel(10);
    //    let (tx_2, mut rx_2) = channel(10);
    //    let (mut pool, _key_1) = KeyedPodPool::singleton(tx_1);
    //    let _key_2 = pool.add(tx_2).unwrap();

    //    let mut fut_1 = pin!(pool.run(sleep_send_and_return(1)));
    //    let mut fut_2 = pin!(pool.run(sleep_send_and_return(2)));
    //    let mut fut_3 = pin!(pool.run(sleep_send_and_return(3)));
    //    let mut cx = Context::from_waker(Waker::noop());

    //    // Polling `fut_1` and `fut_2` advances them to sleep in `sleep_send_and_return`.
    //    // Polling `fut_3` advances until it blocks trying to acquire an object from the pool.
    //    assert!(fut_1.as_mut().poll(&mut cx).is_pending());
    //    assert!(fut_2.as_mut().poll(&mut cx).is_pending());
    //    assert!(fut_3.as_mut().poll(&mut cx).is_pending());

    //    // Run `fut_1` first, which frees up `tx_2` for `fut_3`
    //    // (the second channel was used first due to the stack LIFO semantics).
    //    assert_eq!(fut_1.await, Ok(1));

    //    // Now `fut_3` acquires `tx_2` and advances to the sleep.
    //    assert!(fut_3.as_mut().poll(&mut cx).is_pending());

    //    assert_eq!(fut_3.await, Ok(3));
    //    assert_eq!(fut_2.await, Ok(2));

    //    assert_eq!(rx_2.recv().await, Some(1));
    //    assert_eq!(rx_1.recv().await, Some(2));
    //    assert_eq!(rx_2.recv().await, Some(3));
    //}

    fn sleep_send_and_return(
        response: usize,
    ) -> impl AsyncFnOnce(Arc<AsyncMutex<Sender<usize>>>) -> usize {
        async move |object: Arc<AsyncMutex<Sender<usize>>>| {
            let guard = object.lock().await;
            // Sleep for 1 millisecond just to yield back to the executor.
            // `tokio::task::yield_now` does not guarantee that the same thread won't immediately run again.
            sleep(Duration::from_millis(1)).await;
            guard.deref().send(response).await.unwrap();
            response
        }
    }

    #[tokio::test]
    async fn delete_singleton() {}
}
