use std::error::Error as StdError;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use std::result::Result as StdResult;

/// The [error](StdError) type.
pub struct Error {
    /// Developer-facing message.
    msg: String,

    source: Option<Box<dyn StdError>>,
}

/// A [result](StdResult) that fails with [`Error`](Error).
pub type Result<T> = StdResult<T, Error>;

impl Error {
    /// Return a new error with the given message, but no source (cause) or irritants.
    pub fn leaf<S: Into<String>>(msg: S) -> Self {
        Self {
            msg: msg.into(),
            source: None,
        }
    }

    /// Return a new error with the given message and source (cause), but no or irritants.
    pub fn wrap<S: Into<String>>(msg: S, source: Box<dyn StdError>) -> Self {
        Self {
            msg: msg.into(),
            source: Some(source),
        }
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
