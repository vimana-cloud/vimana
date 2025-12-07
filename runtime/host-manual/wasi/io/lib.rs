mod error;
mod streams;

use error::Error;
use provide::{HostState, Provider, api};

#[api(Error)]
pub struct Io;
