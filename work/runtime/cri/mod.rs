//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! for the Work Node runtime.

use std::collections::HashMap;
use std::result::Result as StdResult;

use anyhow::{anyhow, Error, Result};
use tonic::{Response, Status};

use logging::{log_error, log_error_globally};
use names::{ComponentName, DomainUuid, PodName};

pub(crate) mod image;
pub(crate) mod runtime;

/// Type boilerplate for a typical Tonic response result.
pub type TonicResult<T> = StdResult<Response<T>, Status>;

// These labels must be present on every pod and container using the Vimana handler:

const LABEL_DOMAIN_KEY: &str = "vimana.host/domain";
const LABEL_SERVICE_KEY: &str = "vimana.host/service";
const LABEL_VERSION_KEY: &str = "vimana.host/version";

fn component_name_from_labels(labels: &HashMap<String, String>) -> Result<ComponentName> {
    ComponentName::new(
        DomainUuid::parse(
            labels
                .get(LABEL_DOMAIN_KEY)
                .ok_or(anyhow!("Missing required domain label"))?,
        )?,
        String::from(
            labels
                .get(LABEL_SERVICE_KEY)
                .ok_or(anyhow!("Missing required service label"))?,
        ),
        String::from(
            labels
                .get(LABEL_VERSION_KEY)
                .ok_or(anyhow!("Missing required version label"))?,
        ),
    )
}

trait LogErrorToStatus<T> {
    #[track_caller]
    fn log_error(self, context: impl ErrorLoggingContext) -> StdResult<T, Status>;
}

impl<T> LogErrorToStatus<T> for Result<T> {
    #[track_caller]
    fn log_error(self, context: impl ErrorLoggingContext) -> StdResult<T, Status> {
        self.map_err(|error| {
            context.log(&error);

            for cause in error.chain() {
                if let Some(status) = cause.downcast_ref::<Status>() {
                    return status.clone();
                }
            }
            Status::internal(error.to_string())
        })
    }
}

trait ErrorLoggingContext {
    #[track_caller]
    fn log(&self, error: &Error);
}

struct GlobalLogs;

impl ErrorLoggingContext for GlobalLogs {
    #[track_caller]
    fn log(&self, error: &Error) {
        log_error_globally!("{:?}", error);
    }
}

impl ErrorLoggingContext for &ComponentName {
    #[track_caller]
    fn log(&self, error: &Error) {
        log_error!(component: *self, "{:?}", error);
    }
}

impl ErrorLoggingContext for &PodName {
    #[track_caller]
    fn log(&self, error: &Error) {
        log_error!(pod: *self, "{:?}", error);
    }
}
