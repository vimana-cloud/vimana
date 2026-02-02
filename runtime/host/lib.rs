//! Host functions provided by Vimana.

use anyhow::Result;
use wasmtime::Engine as WasmEngine;
use wasmtime::component::{Linker, ResourceTable};
use wasmtime_wasi::p2::add_to_linker_async as wasip2_add_to_linker_async;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView, add_only_http_to_linker_async};

/// State available to host-defined functions.
pub struct HostState {
    wasi_context: WasiCtx,
    http_context: WasiHttpCtx,
    resource_table: ResourceTable,
}

impl HostState {
    pub fn new() -> Self {
        Self {
            wasi_context: WasiCtx::builder().build(),
            http_context: WasiHttpCtx::new(),
            resource_table: ResourceTable::new(),
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_context,
            table: &mut self.resource_table,
        }
    }
}

impl WasiHttpView for HostState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http_context
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }
}

pub fn grpc_linker(wasmtime: &WasmEngine) -> Result<Linker<HostState>> {
    let mut linker = Linker::new(wasmtime);
    wasip2_add_to_linker_async(&mut linker)?;
    add_only_http_to_linker_async(&mut linker)?;
    Ok(linker)
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}
