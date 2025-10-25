//! Host functions provided by Vimana.

use std::sync::Arc;

use anyhow::Result;
use wasmtime::component::Linker;
use wasmtime::Engine as WasmEngine;

/// State available to host-defined functions.
pub(crate) struct HostState {}

impl HostState {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

pub(crate) mod wasi {
    pub(crate) mod cli {
        pub(crate) mod environment {
            /// Get the POSIX-style environment variables.
            ///
            /// Each environment variable is provided as a pair of string variable names
            /// and string value.
            ///
            /// Morally, these are a value import, but until value imports are available
            /// in the component model, this import function should return the same
            /// values each time it is called.
            pub(crate) async fn get_environment(
                context: wasmtime::StoreContextMut<'_, std::sync::Arc<crate::host::HostState>>,
                parameters: (),
            ) -> anyhow::Result<(Vec<(String, String)>,)> {
                Ok((Vec::new(),))
            }
        }

        pub(crate) mod exit {
            /// Exit the current instance and any linked instances.
            pub(crate) async fn exit(
                context: wasmtime::StoreContextMut<'_, std::sync::Arc<crate::host::HostState>>,
                parameters: (Result<(), ()>,),
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }
    }
}

macro_rules! boxed {
    ($function:expr) => {
        |context, parameters| Box::new($function(context, parameters))
    };
}

pub(crate) fn grpc_linker(wasmtime: &WasmEngine) -> Result<Linker<Arc<HostState>>> {
    let mut linker = Linker::new(wasmtime);

    let mut environment = linker.instance("wasi:cli/environment@0.2.1")?;
    environment.func_wrap_async(
        "get-environment",
        boxed!(wasi::cli::environment::get_environment),
    )?;

    let mut exit = linker.instance("wasi:cli/exit@0.2.1")?;
    exit.func_wrap_async("exit", boxed!(wasi::cli::exit::exit))?;

    Ok(linker)
}
