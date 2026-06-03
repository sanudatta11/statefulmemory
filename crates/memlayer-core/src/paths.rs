//! Filesystem path resolution for the memlayer data directory.
//!
//! Default layout (PRD §0):
//!
//! ```text
//! ~/.memlayer/
//!   daemon.sock            # UDS endpoint (mode 0600)
//!   daemon.lock            # advisory flock for spawn races
//!   daemon.pid             # daemon PID
//!   daemon.log             # tracing-appender JSON log (10 MB × 5)
//!   tokens.db              # bearer-token store (TCP mode)
//!   config.json            # global daemon config (optional)
//!   projects/
//!     <normalized>.db      # per-project SQLite + FTS5
//!     <normalized>/        # per-project directory (config.json, etc.)
//!       config.json
//!   diagnostics-<ts>.json  # produced on SIGUSR1
//! ```
//!
//! Override via `MEMLAYER_DATA_DIR`.

use std::path::{Path, PathBuf};

/// Returns the resolved data directory.
///
/// Resolution order (PRD §13):
/// 1. `MEMLAYER_DATA_DIR` env var.
/// 2. `~/.memlayer/`.
///
/// Panics only if the OS does not expose a home directory and `MEMLAYER_DATA_DIR`
/// is unset. In practice that never happens on the supported platforms (macOS, Linux).
pub fn data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("MEMLAYER_DATA_DIR") {
        return PathBuf::from(custom);
    }
    let home = dirs::home_dir().expect("HOME directory unavailable; set MEMLAYER_DATA_DIR");
    home.join(".memlayer")
}

pub fn projects_dir() -> PathBuf {
    data_dir().join("projects")
}

/// Returns the absolute path to a project's SQLite file.
pub fn project_db_path(normalized_id: &str) -> PathBuf {
    projects_dir().join(format!("{normalized_id}.db"))
}

/// Returns the directory that holds the project's `config.json`.
pub fn project_dir(normalized_id: &str) -> PathBuf {
    projects_dir().join(normalized_id)
}

pub fn project_config_path(normalized_id: &str) -> PathBuf {
    project_dir(normalized_id).join("config.json")
}

pub fn socket_path() -> PathBuf {
    data_dir().join("daemon.sock")
}

pub fn lock_path() -> PathBuf {
    data_dir().join("daemon.lock")
}

pub fn pid_path() -> PathBuf {
    data_dir().join("daemon.pid")
}

pub fn log_path() -> PathBuf {
    data_dir().join("daemon.log")
}

pub fn tokens_db_path() -> PathBuf {
    data_dir().join("tokens.db")
}

/// Diagnostics dump path with a UTC RFC-3339 timestamp suffix (no colons, file-system safe).
pub fn diagnostics_path(ts: &chrono::DateTime<chrono::Utc>) -> PathBuf {
    let ts = ts.format("%Y%m%dT%H%M%SZ");
    data_dir().join(format!("diagnostics-{ts}.json"))
}

/// Best-effort `mkdir -p` of the data directory and its `projects/` child.
pub fn ensure_dirs(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::create_dir_all(dir.join("projects"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MEMLAYER_DATA_DIR` is a process-global env var; the two tests below
    /// both mutate it. Without a shared lock the default cargo-test parallel
    /// runner interleaves their `set_var` calls and one of them reads the
    /// other's value. Serialize them through this mutex.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn data_dir_respects_env_var() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("MEMLAYER_DATA_DIR", "/tmp/memlayer-test-xyz");
        assert_eq!(data_dir(), PathBuf::from("/tmp/memlayer-test-xyz"));
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn project_paths_compose() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("MEMLAYER_DATA_DIR", "/tmp/memlayer-paths");
        assert_eq!(
            project_db_path("foo"),
            PathBuf::from("/tmp/memlayer-paths/projects/foo.db"),
        );
        assert_eq!(
            project_config_path("foo"),
            PathBuf::from("/tmp/memlayer-paths/projects/foo/config.json"),
        );
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }
}
