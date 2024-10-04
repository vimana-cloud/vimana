use std::error::Error as StdError;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use std::result::Result as StdResult;
use tonic::{Code, Status};

/// The [error](StdError) type.
pub struct Error {
    /// Developer-facing message.
    pub msg: String,

    /// Developer-facing Parent cause of this error.
    source: Option<Box<dyn StdError>>,

    /// User-facing gRPC status code associated with this error.
    code: Code,
}

/// A [result](StdResult) that fails with [`Error`](Error).
pub type Result<T> = StdResult<T, Error>;

impl Error {
    /// Return a new error with the given message, but no source (cause) or irritants.
    pub fn leaf<S: Into<String>>(msg: S) -> Self {
        Self {
            msg: msg.into(),
            source: None,
            code: Code::Internal,
        }
    }

    /// Return a new error with the given message and source (cause), but no or irritants.
    pub fn wrap<S: Into<String>, E: StdError + 'static>(msg: S, source: E) -> Self {
        Self {
            msg: msg.into(),
            source: Some(Box::new(source)),
            code: Code::Internal,
        }
    }

    /// Consume self and return a gRPC status with the same message and given status code.
    pub fn to_status(self, code: Code) -> Status {
        Status::new(code, self.msg)
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

impl From<Error> for Status {
    fn from(e: Error) -> Status {
        Status::new(e.code, e.msg)
    }
}
