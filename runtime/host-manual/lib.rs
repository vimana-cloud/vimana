//! Host functions provided by Vimana.

use std::sync::Arc;

use anyhow::Result;
use wasmtime::Engine as WasmEngine;
use wasmtime::component::Linker;

pub use provide::HostState;
use provide::{Provider, api};
use wasi::Wasi;

#[api(Wasi)]
pub struct Host;

mod wusi {
    mod cli {
        mod environment {
            /// Get the POSIX-style environment variables.
            ///
            /// Each environment variable is provided as a pair of string variable names
            /// and string value.
            ///
            /// Morally, these are a value import, but until value imports are available
            /// in the component model, this import function should return the same
            /// values each time it is called.
            async fn get_environment(
                _context: wasmtime::StoreContextMut<'_, std::sync::Arc<provide::HostState>>,
                _parameters: (),
            ) -> anyhow::Result<(Vec<(String, String)>,)> {
                Ok((Vec::new(),))
            }
        }

        mod exit {
            /// Exit the current instance and any linked instances.
            async fn exit(
                _context: wasmtime::StoreContextMut<'_, std::sync::Arc<provide::HostState>>,
                _parameters: (Result<(), ()>,),
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }
    }
}

pub fn grpc_linker(wasmtime: &WasmEngine) -> Result<Linker<Arc<HostState>>> {
    let mut linker = Linker::new(wasmtime);
    Host.provide(&mut linker)?;
    Ok(linker)
}
