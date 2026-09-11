//! The single error type used across memlayer crates.
//!
//! Maps cleanly to gRPC status codes per PRD §6.2.

use std::io;
use thiserror::Error;

/// The full error type used by memlayer. Variants are designed so that the
/// daemon's gRPC layer can project them onto `tonic::Status` without losing
/// the structured `code`/`message`/`details` triple from PRD §6.2.
#[derive(Debug, Error)]
pub enum Error {
    /// Caller-supplied input failed validation.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The named resource was not present.
    #[error("not found: {0}")]
    NotFound(String),

    /// A unique-key violation: the resource already exists.
    #[error("already exists: {0}")]
    AlreadyExists(String),

    /// A precondition for the operation was not met.
    #[error("failed precondition: {0}")]
    FailedPrecondition(String),

    /// The daemon is shutting down or backpressured.
    #[error("unavailable: {0}")]
    Unavailable(String),

    /// The daemon is in read-only mode (disk-full).
    #[error("resource exhausted: {0}")]
    ResourceExhausted(String),

    /// Caller authenticated but lacks the required permission (admin-only RPC).
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Caller did not authenticate (TCP mode without a valid bearer token).
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    /// Internal daemon error. Backtraces stay server-side; clients see message only.
    #[error("internal error: {0}")]
    Internal(String),

    /// Wraps an underlying `io::Error`. Always treated as `Internal` on the wire.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// Wraps a JSON serialization or parse failure. `INVALID_ARGUMENT` on the wire
    /// when the JSON is caller-supplied; `INTERNAL` otherwise (caller decides).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Coarse classification of an error, used to choose the gRPC status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidArgument,
    NotFound,
    AlreadyExists,
    FailedPrecondition,
    Unavailable,
    ResourceExhausted,
    PermissionDenied,
    Unauthenticated,
    Internal,
}

impl Error {
    /// Stable string code (used in `google.rpc.ErrorInfo.reason` payloads).
    pub fn code(&self) -> &'static str {
        match self.kind() {
            ErrorKind::InvalidArgument => "INVALID_ARGUMENT",
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::AlreadyExists => "ALREADY_EXISTS",
            ErrorKind::FailedPrecondition => "FAILED_PRECONDITION",
            ErrorKind::Unavailable => "UNAVAILABLE",
            ErrorKind::ResourceExhausted => "RESOURCE_EXHAUSTED",
            ErrorKind::PermissionDenied => "PERMISSION_DENIED",
            ErrorKind::Unauthenticated => "UNAUTHENTICATED",
            ErrorKind::Internal => "INTERNAL",
        }
    }

    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::InvalidArgument(_) => ErrorKind::InvalidArgument,
            Error::NotFound(_) => ErrorKind::NotFound,
            Error::AlreadyExists(_) => ErrorKind::AlreadyExists,
            Error::FailedPrecondition(_) => ErrorKind::FailedPrecondition,
            Error::Unavailable(_) => ErrorKind::Unavailable,
            Error::ResourceExhausted(_) => ErrorKind::ResourceExhausted,
            Error::PermissionDenied(_) => ErrorKind::PermissionDenied,
            Error::Unauthenticated(_) => ErrorKind::Unauthenticated,
            Error::Internal(_) | Error::Io(_) | Error::Json(_) => ErrorKind::Internal,
        }
    }

    pub fn invalid<S: Into<String>>(msg: S) -> Self {
        Error::InvalidArgument(msg.into())
    }

    pub fn not_found<S: Into<String>>(msg: S) -> Self {
        Error::NotFound(msg.into())
    }

    pub fn internal<S: Into<String>>(msg: S) -> Self {
        Error::Internal(msg.into())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_argument_code() {
        assert_eq!(Error::invalid("x").code(), "INVALID_ARGUMENT");
    }

    #[test]
    fn io_error_is_internal() {
        let e: Error = io::Error::other("boom").into();
        assert_eq!(e.kind(), ErrorKind::Internal);
        assert_eq!(e.code(), "INTERNAL");
    }
}
