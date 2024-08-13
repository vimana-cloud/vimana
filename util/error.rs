
use std::error::Error as StdError;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};

pub struct Error {
    msg: String,
    source: Option<Box<dyn StdError>>,
}

impl Error {
    pub fn leaf(msg: String) -> Self {
        Self { msg, source: None }
    }
    pub fn wrap(msg: String, source: Box<dyn StdError>) -> Self {
        Self { msg, source: Some(source) }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_deref()
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "\"{}\"", self.msg)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.msg)
    }
}