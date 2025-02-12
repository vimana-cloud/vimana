//! Host functions provided by Vimana.

use std::future::Future;
use std::sync::Arc;

use tonic::Code;
use wasmtime::component::{ComponentNamedList, Lift, Linker, LinkerInstance, Lower};
use wasmtime::Engine as WasmEngine;
use wasmtime::StoreContextMut;

use error::{log_error_status, Result};
use names::ComponentName;

/// State available to host-defined functions.
pub struct HostState {}

impl HostState {
    pub fn new() -> Self {
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
                context: wasmtime::StoreContextMut<'_, std::sync::Arc<crate::HostState>>,
                parameters: (),
            ) -> anyhow::Result<(Vec<(String, String)>,)> {
                Ok((Vec::new(),))
            }
        }

        pub(crate) mod exit {
            /// Exit the current instance and any linked instances.
            pub(crate) async fn exit(
                context: wasmtime::StoreContextMut<'_, std::sync::Arc<crate::HostState>>,
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

pub fn grpc_linker(
    component: &ComponentName,
    wasmtime: &WasmEngine,
) -> Result<Linker<Arc<HostState>>> {
    let mut linker = Linker::new(wasmtime);

    let mut environment = instance(&mut linker, "wasi:cli/environment@0.2.1", component)?;
    link_function(
        &mut environment,
        "get-environment",
        boxed!(wasi::cli::environment::get_environment),
        component,
    )?;

    let mut exit = instance(&mut linker, "wasi:cli/exit@0.2.1", component)?;
    link_function(&mut exit, "exit", boxed!(wasi::cli::exit::exit), component)?;

    Ok(linker)
}

fn instance<'a, 'b>(
    linker: &'a mut Linker<Arc<HostState>>,
    name: &'b str,
    component: &ComponentName,
) -> Result<LinkerInstance<'a, Arc<HostState>>> {
    linker.instance(name).map_err(
        // Name conflict.
        log_error_status!(Code::Internal, "link-instance-name-conflict", component),
    )
}

fn link_function<F, Params, Return>(
    linker: &mut LinkerInstance<'_, Arc<HostState>>,
    name: &str,
    implementation: F,
    component: &ComponentName,
) -> Result<()>
where
    F: for<'a> Fn(
            StoreContextMut<'a, Arc<HostState>>,
            Params,
        ) -> Box<dyn Future<Output = anyhow::Result<Return>> + Send + 'a>
        + Send
        + Sync
        + 'static,
    Params: ComponentNamedList + Lift + 'static,
    Return: ComponentNamedList + Lower + 'static,
{
    linker.func_wrap_async(name, implementation).map_err(
        // Name conflict.
        log_error_status!(Code::Internal, "link-function-name-conflict", component),
    )
}
