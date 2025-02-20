//! Error handling for the Work runtime.
//!
//! All processes performed by the Work runtime are initiated with a gRPC call from Kubelet
//! &mdash; to either `runtime.v1.RuntimeService` or `runtime.v1.ImageService` &mdash;
//! so errors can always be reported in one of two ways:
//! 1. As a gRPC error status response to Kubelet.
//! 2. To the runtime logs (`journald -u workd`)
//!
//! End users do not interact with the Work runtime directly;
//! all user-facing errors go through the API.
//! The Kubelet is the intended audience for error messages from the Work runtime.
//!
//! In light of all this, follow these error-handling practices in the Work runtime:
//! - Any fallible method should return `Result<_, Status>`
//!   for easy fail-fast checks (`?`) down all call stacks.
//! - All error statuses must include a short, simple, unique, static message
//!   *e.g.* `component-compilation-error`.
//!   Might as well use a descriptive status code, too.
//!   This information ends up in Kubelet logs and is generally of limited debugging use;
//!   only intended to correlate those statuses with source code logic and the runtime logs.
//! - Any other relevant information must be logged using one of the macros in this crate,
//!   where `target` is the same short, static message that was returned in the status.
//!   See *e.g.* [`log_error_status`] for a macro that combines the two.
//!   This enables correlating runtime logs with Kubelet logs
//!   while collecting all event
//!   &mdash; which enforce providing the predictable structured fields
//!   used to safeguard log access. &mdash;

use std::result::Result as StdResult;

pub use log::{log as _log, Level};
pub use tonic::{Code, Status};

/// Shorthand for results with [`Status`] error types.
pub type Result<T> = StdResult<T, Status>;

/// The most basic requirements for emitting a log:
/// - Log level.
/// - Name: simple, static, unique to the source code location
///   (possibly shared with a [`Status`] or adjacent log messages).
/// - Component: all log messages occur in the context of a component name
///   for the purpose of organizing and controlling log access.
/// - irritant: the thing to log (using [`Debug`] formatting).
#[macro_export]
macro_rules! log {
    ($level: expr, $target:expr, $component:expr, $irritant:expr) => {{
        // Check the type of `$component` by moving the reference.
        let component: &ComponentName = $component;
        $crate::_log!(
            target: $target,
            $level,
            domain = component.service.domain.to_string().as_str(),
            service = &component.service.service.as_str(),
            version = &component.version.as_str();
            "{:?}",
            $irritant,
        );
    }};
}

#[macro_export]
macro_rules! log_error {
    ($target:expr, $component:expr, $irritant:expr) => {
        $crate::log!($crate::Level::Error, $target, $component, $irritant)
    };
}

#[macro_export]
macro_rules! log_warn {
    ($target:expr, $component:expr, $irritant:expr) => {
        $crate::log!($crate::Level::Warn, $target, $component, $irritant)
    };
}

#[macro_export]
macro_rules! log_info {
    ($target:expr, $component:expr, $irritant:expr) => {
        $crate::log!($crate::Level::Info, $target, $component, $irritant)
    };
}

#[macro_export]
macro_rules! log_error_status {
    ($code:expr, $target:expr, $component:expr) => {
        |irritant| {
            $crate::log_error!($target, $component, irritant);
            $crate::Status::new($code, $target)
        }
    };
    ($target:expr, $component:expr) => {
        log_error_status!($crate::Code::Internal, $target, $component)
    };
}
