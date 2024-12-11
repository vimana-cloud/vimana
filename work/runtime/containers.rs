//! Caching container store client for Vimana containers
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! for the Work Node runtime.

use container_proto::work::runtime::container::Container;
use error::Result;
use names::ComponentName;

pub struct ContainerStore {}

impl ContainerStore {
    pub fn new() -> Self {
        ContainerStore {}
    }

    pub fn get_container(&self, _name: &ComponentName) -> Result<Container> {
        todo!()
    }
}
