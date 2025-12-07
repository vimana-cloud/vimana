//! https://github.com/WebAssembly/WASI/blob/v0.2.8/wasip2/io/error.wit

use std::sync::Arc;

use anyhow::Result;
use wasmtime::StoreContextMut;
use wasmtime::component::Resource;

use provide::{HostState, Provider, function, instance, resource};

pub(crate) struct Error;

struct ErrorResource {}

#[instance(
    "wasi:io/error@0.2.0",
    "wasi:io/error@0.2.1",
    "wasi:io/error@0.2.2",
    "wasi:io/error@0.2.3",
    "wasi:io/error@0.2.4",
    "wasi:io/error@0.2.5",
    "wasi:io/error@0.2.6",
    "wasi:io/error@0.2.7",
    "wasi:io/error@0.2.8"
)]
impl Error {
    #[resource("error", ErrorResource)]
    async fn error_destructor(
        _context: StoreContextMut<'_, Arc<HostState>>,
        _index: u32,
    ) -> Result<()> {
        Ok(())
    }

    #[function("[method]error.to-debug-string")]
    async fn error_to_debug_string(
        _context: StoreContextMut<'_, Arc<HostState>>,
        _resource: (Resource<ErrorResource>,),
    ) -> Result<(String,)> {
        // TODO: Figure out real error handling.
        Ok((String::from("error"),))
    }
}
