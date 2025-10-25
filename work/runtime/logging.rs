//! Logging for the Work runtime.
//!
//! Logs are the primary debugging tool for work nodes.
//! All logged information must be tagged with the relevant component / pod name
//! so the logs can be filtered for tenant privacy.

#[doc(hidden)]
pub use tracing::{event, Level};

/// The most basic requirements for emitting a log:
/// - Log level.
/// - Component or pod: all log messages occur in the context of a component (or pod) name
///   for the purpose of organizing and controlling log access.
/// - Arguments: A literal format string, followed by optional irritants, for the message.
#[macro_export]
macro_rules! log {
    ($level:expr, component: $component:expr, $($arg:tt)+) => {{
        // Check the type of `$component` by moving the reference.
        let component: &ComponentName = $component;
        $crate::event!(
            $level,
            domain = component.server.domain.to_string(),
            server = component.server.server,
            version = component.version,
            $($arg)+
        );
    }};
    ($level:expr, pod: $pod:expr, $($arg:tt)+) => {{
        // Check the type of `$pod` by moving the reference.
        let pod: &PodName = $pod;
        $crate::event!(
            $level,
            domain = pod.component.server.domain.to_string(),
            server = pod.component.server.server,
            version = pod.component.version,
            pod = pod.pod.to_string(),
            $($arg)+
        );
    }};
}

#[macro_export]
macro_rules! log_error {
    (component: $component:expr, $($arg:tt)+) => {
        $crate::log!($crate::Level::ERROR, component: $component, $($arg)+)
    };
    (pod: $pod:expr, $($arg:tt)+) => {
        $crate::log!($crate::Level::ERROR, pod: $pod, $($arg)+)
    };
}

/// Log an error when there really is no relevant component or pod name to use as context,
/// such as when logging so early during the life of an RPC
/// that no name has been successfully parsed.
/// Always use [`log_error`] instead if possible.
#[macro_export]
macro_rules! log_error_globally {
    ($($arg:tt)+) => {
        $crate::event!($crate::Level::ERROR, $($arg)+);
    };
}

#[macro_export]
macro_rules! log_warn {
    (component: $component:expr, $($arg:tt)+) => {
        $crate::log!($crate::Level::WARN, component: $component, $($arg)+)
    };
    (pod: $pod:expr, $($arg:tt)+) => {
        $crate::log!($crate::Level::WARN, pod: $pod, $($arg)+)
    };
}

#[macro_export]
macro_rules! log_info {
    (component: $component:expr, $($arg:tt)+) => {
        $crate::log!($crate::Level::INFO, component: $component, $($arg)+)
    };
    (pod: $pod:expr, $($arg:tt)+) => {
        $crate::log!($crate::Level::INFO, pod: $pod, $($arg)+)
    };
}

/// Log normal runtime information
/// when there really is no relevant component or pod name to use as context,
/// such as when behavior relevant to the system as a whole but not to any individual component.
/// Always use [`log_info`] instead if possible.
#[macro_export]
macro_rules! log_info_globally {
    ($($arg:tt)+) => {
        $crate::event!($crate::Level::INFO, $($arg)+);
    };
}
