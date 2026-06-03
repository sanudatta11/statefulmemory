//! Map gRPC status codes to CLI exit codes (PRD §3.4, FR13).
//!
//! ```text
//! 0  success
//! 1  general failure
//! 2  usage error
//! 3  already in target state
//! 4  daemon not reachable / auto-spawn failed
//! 5  ambiguous project (recoverable: pass --project)
//! ```
//!
//! Exit code 4 is owned by [`crate::autospawn`] (timeout path); this module
//! handles the after-the-RPC-was-issued mapping.

use tonic::Code;

pub const SUCCESS: u8 = 0;
pub const GENERAL: u8 = 1;
pub const USAGE: u8 = 2;
pub const ALREADY_IN_STATE: u8 = 3;
pub const DAEMON_UNREACHABLE: u8 = 4;
pub const AMBIGUOUS_PROJECT: u8 = 5;

/// Default mapping from a tonic status code to an exit code.
///
/// Specific commands override this — `daemon start` maps `AlreadyExists` to 3
/// (SC-3), but for an `obs save` an `AlreadyExists` is a duplicate sync_id
/// and exits 1.
pub fn from_status(code: Code) -> u8 {
    match code {
        Code::Ok => SUCCESS,
        Code::InvalidArgument => USAGE,
        Code::NotFound => GENERAL,
        Code::AlreadyExists => GENERAL,
        Code::FailedPrecondition => GENERAL,
        Code::Unavailable => GENERAL,
        Code::ResourceExhausted => GENERAL,
        Code::PermissionDenied => GENERAL,
        Code::Unauthenticated => GENERAL,
        Code::Internal => GENERAL,
        // Cancelled / DeadlineExceeded / OutOfRange / Aborted / Unimplemented
        // / Unknown / DataLoss all surface as general failures from the
        // CLI's point of view.
        _ => GENERAL,
    }
}

/// Per-command override: `daemon start` and `daemon restart` distinguish
/// "already running" (exit 3) from a real failure.
pub fn for_daemon_lifecycle(code: Code) -> u8 {
    match code {
        Code::Ok => SUCCESS,
        Code::AlreadyExists | Code::FailedPrecondition => ALREADY_IN_STATE,
        other => from_status(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skeleton test for spec2-t2: every status code in the matrix maps to a
    /// stable exit code. Update this table when commands gain custom mappings.
    #[test]
    fn status_to_exit_code_table() {
        let cases: &[(Code, u8)] = &[
            (Code::Ok, SUCCESS),
            (Code::InvalidArgument, USAGE),
            (Code::NotFound, GENERAL),
            (Code::AlreadyExists, GENERAL),
            (Code::FailedPrecondition, GENERAL),
            (Code::Unavailable, GENERAL),
            (Code::ResourceExhausted, GENERAL),
            (Code::PermissionDenied, GENERAL),
            (Code::Unauthenticated, GENERAL),
            (Code::Internal, GENERAL),
            (Code::Cancelled, GENERAL),
            (Code::DeadlineExceeded, GENERAL),
            (Code::Unimplemented, GENERAL),
        ];
        for (code, want) in cases {
            assert_eq!(from_status(*code), *want, "from_status({code:?})");
        }
    }

    #[test]
    fn daemon_lifecycle_maps_already_to_three() {
        assert_eq!(for_daemon_lifecycle(Code::AlreadyExists), ALREADY_IN_STATE);
        assert_eq!(for_daemon_lifecycle(Code::FailedPrecondition), ALREADY_IN_STATE);
    }

    #[test]
    fn daemon_lifecycle_maps_other_via_default() {
        assert_eq!(for_daemon_lifecycle(Code::InvalidArgument), USAGE);
        assert_eq!(for_daemon_lifecycle(Code::Unavailable), GENERAL);
    }

    #[test]
    fn exit_code_constants_match_prd() {
        assert_eq!(SUCCESS, 0);
        assert_eq!(GENERAL, 1);
        assert_eq!(USAGE, 2);
        assert_eq!(ALREADY_IN_STATE, 3);
        assert_eq!(DAEMON_UNREACHABLE, 4);
        assert_eq!(AMBIGUOUS_PROJECT, 5);
    }
}
