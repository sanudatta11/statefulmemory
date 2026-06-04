//! Convert `memlayer_core::Error` → `tonic::Status`.
//!
//! Spec sections: FR12, EH-1..3.
//!
//! Wire shape (PRD §6.2):
//!   - status code from `ErrorKind`
//!   - status message = error's display string
//!
//! An earlier revision also packed a `google.rpc.ErrorInfo` into the
//! `memlayer-error-info-bin` metadata header. That has been removed
//! because (a) no client reads it and (b) every codepath that builds
//! the metadata risked tripping tonic's `-bin` key validation, which
//! panicked the daemon in the middle of an error response and surfaced
//! to clients as `h2 protocol error: http2 error` instead of the real
//! status. If a client ever needs structured error details, reintroduce
//! the metadata via `MetadataMap::insert_bin` with a `MetadataValue<Binary>`
//! and add coverage that exercises every `ErrorKind` to guard against
//! the panic.

use memlayer_core::error::{Error, ErrorKind};
use tonic::Status;

pub fn to_status(err: Error) -> Status {
    let kind = err.kind();
    let message = err.to_string();
    match kind {
        ErrorKind::InvalidArgument => Status::invalid_argument(message),
        ErrorKind::NotFound => Status::not_found(message),
        ErrorKind::AlreadyExists => Status::already_exists(message),
        ErrorKind::FailedPrecondition => Status::failed_precondition(message),
        ErrorKind::Unavailable => Status::unavailable(message),
        ErrorKind::ResourceExhausted => Status::resource_exhausted(message),
        ErrorKind::PermissionDenied => Status::permission_denied(message),
        ErrorKind::Unauthenticated => Status::unauthenticated(message),
        ErrorKind::Internal => Status::internal(message),
    }
}

/// Helper: short-circuit Result<T, memlayer::Error> → Result<T, tonic::Status>.
pub fn map<T>(r: memlayer_core::Result<T>) -> Result<T, Status> {
    r.map_err(to_status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    /// Regression: every error variant must produce a clean tonic Status
    /// without panic. The previous revision inserted custom metadata via
    /// the wrong API for `-bin` keys, which panicked the daemon mid-error
    /// and surfaced as a generic h2 protocol error. Sweeping every
    /// ErrorKind here guarantees no future change can reintroduce that
    /// failure mode silently.
    #[test]
    fn every_error_kind_builds_status_without_panic() {
        let cases: &[(Error, Code)] = &[
            (Error::invalid("bad arg"), Code::InvalidArgument),
            (Error::not_found("missing"), Code::NotFound),
            (Error::AlreadyExists("dup".to_string()), Code::AlreadyExists),
            (
                Error::FailedPrecondition("not ready".to_string()),
                Code::FailedPrecondition,
            ),
            (Error::Unavailable("backpressured".to_string()), Code::Unavailable),
            (
                Error::ResourceExhausted("disk full".to_string()),
                Code::ResourceExhausted,
            ),
            (
                Error::PermissionDenied("forbidden".to_string()),
                Code::PermissionDenied,
            ),
            (
                Error::Unauthenticated("no token".to_string()),
                Code::Unauthenticated,
            ),
            (Error::internal("boom"), Code::Internal),
        ];
        for (err, expected_code) in cases {
            // We have to clone because Error is not Copy and the table
            // entries above borrow.
            let cloned = match err {
                Error::InvalidArgument(s) => Error::invalid(s.clone()),
                Error::NotFound(s) => Error::not_found(s.clone()),
                Error::AlreadyExists(s) => Error::AlreadyExists(s.clone()),
                Error::FailedPrecondition(s) => Error::FailedPrecondition(s.clone()),
                Error::Unavailable(s) => Error::Unavailable(s.clone()),
                Error::ResourceExhausted(s) => Error::ResourceExhausted(s.clone()),
                Error::PermissionDenied(s) => Error::PermissionDenied(s.clone()),
                Error::Unauthenticated(s) => Error::Unauthenticated(s.clone()),
                Error::Internal(s) => Error::internal(s.clone()),
                _ => unreachable!("table entries above only cover named variants"),
            };
            let status = to_status(cloned);
            assert_eq!(status.code(), *expected_code, "code mismatch for {err:?}");
            assert!(!status.message().is_empty(), "message empty for {err:?}");
        }
    }
}
