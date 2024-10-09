/// Caching OCI client for Vimana containers
/// [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
/// for the Work Node runtime.
use error::{Error, Result};
use names::FullVersionName;
use pods_proto::pods::PodConfig;
use pool::Pod;

pub struct ContainerStore {}

impl ContainerStore {
    pub fn new() -> Self {
        ContainerStore {}
    }

    pub fn new_container(&self, name: &FullVersionName) -> Result<(PodConfig, Pod)> {
        todo!()
    }
}
