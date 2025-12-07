//! https://github.com/WebAssembly/WASI/blob/v0.2.8/wasip2/io/streams.wit

use std::sync::Arc;

use anyhow::Result;
use wasmtime::StoreContextMut;
use wasmtime::component::Resource;

use provide::{HostState, Provider, function, instance, resource};

pub(crate) struct Streams;

struct OutputStream {}

#[instance(
    "wasi:io/streams@0.2.0",
    "wasi:io/streams@0.2.1",
    "wasi:io/streams@0.2.2",
    "wasi:io/streams@0.2.3",
    "wasi:io/streams@0.2.4",
    "wasi:io/streams@0.2.5",
    "wasi:io/streams@0.2.6",
    "wasi:io/streams@0.2.7",
    "wasi:io/streams@0.2.8"
)]
impl Streams {
    #[resource("output-stream", OutputStream)]
    async fn output_stream_destructor(
        _context: StoreContextMut<'_, Arc<HostState>>,
        _index: u32,
    ) -> Result<()> {
        Ok(())
    }

    #[function("[method]output-stream.check-write")]
    async fn output_stream_check_write(
        _context: StoreContextMut<'_, Arc<HostState>>,
        _resource: (Resource<OutputStream>,),
    ) -> Result<(u64,)> {
        // Return a reasonable buffer size for writing.
        Ok((1024 * 1024,))
    }
}
