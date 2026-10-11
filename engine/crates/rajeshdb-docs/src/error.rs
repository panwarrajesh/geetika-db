use std::fmt;
use std::io;

#[derive(Debug)]
pub enum DocError {
    Io(io::Error),
    InvalidArgument(String),
    NotFound(String),
    AlreadyExists(String),
    SchemaViolation(String),
}

pub type Result<T> = std::result::Result<T, DocError>;

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocError::Io(e) => write!(f, "storage error: {}", e),
            DocError::InvalidArgument(m) => write!(f, "invalid argument: {}", m),
            DocError::NotFound(m) => write!(f, "not found: {}", m),
            DocError::AlreadyExists(m) => write!(f, "already exists: {}", m),
            DocError::SchemaViolation(m) => write!(f, "schema violation: {}", m),
        }
    }
}

impl std::error::Error for DocError {}

impl From<io::Error> for DocError {
    fn from(e: io::Error) -> Self {
        DocError::Io(e)
    }
}

impl From<serde_json::Error> for DocError {
    fn from(e: serde_json::Error) -> Self {
        DocError::InvalidArgument(format!("JSON error: {}", e))
    }
}

pub(crate) fn invalid(msg: impl Into<String>) -> DocError {
    DocError::InvalidArgument(msg.into())
}

pub(crate) fn corrupt(msg: impl Into<String>) -> DocError {
    DocError::Io(io::Error::new(io::ErrorKind::InvalidData, msg.into()))
}
