use io::Io;
use provide::{HostState, Provider, api};

#[api(Io)]
pub struct Wasi;
