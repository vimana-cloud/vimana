//! Implementation of the
//! [Container Runtime Interface](https://kubernetes.io/docs/concepts/architecture/cri/)
//! for the Work Node runtime.

use std::collections::HashMap;

use tonic::{Response, Status};

use names::{ComponentName, DomainUuid};

pub(crate) mod image;
pub(crate) mod runtime;

/// Type boilerplate for a typical Tonic response result.
pub type TonicResult<T> = Result<Response<T>, Status>;

// These labels must be present on every pod and container using the Vimana handler:

const LABEL_DOMAIN_KEY: &str = "vimana.host/domain";
const LABEL_SERVICE_KEY: &str = "vimana.host/service";
const LABEL_VERSION_KEY: &str = "vimana.host/version";

fn component_name_from_labels(labels: &HashMap<String, String>) -> Result<ComponentName, Status> {
    ComponentName::new(
        DomainUuid::parse(
            labels
                .get(LABEL_DOMAIN_KEY)
                .ok_or_else(|| Status::invalid_argument("expected-domain-label"))?,
        )?,
        String::from(
            labels
                .get(LABEL_SERVICE_KEY)
                .ok_or_else(|| Status::invalid_argument("expected-service-label"))?,
        ),
        String::from(
            labels
                .get(LABEL_VERSION_KEY)
                .ok_or_else(|| Status::invalid_argument("expected-version-label"))?,
        ),
    )
}
