use std::sync::Arc;

use anyhow::Result;
use wasmtime::component::Linker;

pub use provide_macro::{api, function, instance, resource};

/// State available to host-defined functions.
pub struct HostState {}

pub trait Provider {
    fn provide(&self, linker: &mut Linker<Arc<HostState>>) -> Result<()>;
}

impl HostState {
    pub fn new() -> Self {
        Self {}
    }
}
