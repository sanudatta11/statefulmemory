//! Daemon auto-spawn re-exports.
//!
//! Implementation lives in [`statefulmemory_client::ensure`] so the MCP server
//! and CLI share one probe-and-repair path. Exit-code mapping stays here to
//! keep FR13 constants next to the CLI exit module.

pub use statefulmemory_client::ensure::{
    ensure_running, poll_socket, probe, AutoSpawnConfig, AutoSpawnError, EnsureOutcome,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn timeout_maps_to_exit_4() {
        let err = AutoSpawnError::Timeout {
            socket: PathBuf::from("/tmp/x.sock"),
            budget: Duration::from_secs(5),
            log: PathBuf::from("/tmp/daemon.log"),
        };
        assert_eq!(err.exit_code(), crate::exit::DAEMON_UNREACHABLE);
        assert_eq!(err.exit_code(), 4);
    }
}
