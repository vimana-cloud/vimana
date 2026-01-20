//! Host functions provided by Vimana.

use anyhow::Result;
use wasmtime::Engine as WasmEngine;
use wasmtime::component::{Linker, ResourceTable};
use wasmtime_wasi::p2::add_to_linker_async as wasip2_add_to_linker_async;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

/// State available to host-defined functions.
pub struct HostState {
    context: WasiCtx,
    resource_table: ResourceTable,
}

impl HostState {
    pub fn new() -> Self {
        Self {
            context: WasiCtx::builder().build(),
            resource_table: ResourceTable::new(),
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.context,
            table: &mut self.resource_table,
        }
    }
}

pub fn grpc_linker(wasmtime: &WasmEngine) -> Result<Linker<HostState>> {
    let mut linker = Linker::new(wasmtime);
    wasip2_add_to_linker_async(&mut linker)?;
    Ok(linker)
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}
