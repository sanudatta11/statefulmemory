//! Daemon auto-spawn: socket connect → flock → spawn detached → poll.
//!
//! Spec sections: FR2, PRD §4.1, EH-7, SC-1, SC-2.
//!
//! Algorithm (per PRD §4.1):
//! 1. Try to connect to the daemon socket.
//! 2. If missing or connect-refused, acquire `flock` on `daemon.lock`.
//! 3. Re-check the socket (another CLI may have raced and spawned).
//! 4. If still missing, `Command::new(self_path()).arg("daemon").arg("start")`
//!    detached (setsid + closed stdio).
//! 5. Poll for socket readiness with a 5 s budget and exponential backoff
//!    (10 ms → 640 ms cap). Release the lock when ready.
//! 6. On timeout, return [`AutoSpawnError::Timeout`] — caller exits 4 with a
//!    diagnostic pointing at `daemon.log`.

use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum AutoSpawnError {
    #[error("daemon not reachable: socket never appeared at {socket} within {budget:?}; check {log}")]
    Timeout { socket: PathBuf, budget: Duration, log: PathBuf },

    #[error("could not acquire spawn lock at {0}: {1}")]
    Lock(PathBuf, std::io::Error),

    #[error("could not spawn daemon at {0}: {1}")]
    Spawn(PathBuf, std::io::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl AutoSpawnError {
    /// Map to FR13 exit code 4 for timeouts; other failures are general (1).
    pub fn exit_code(&self) -> u8 {
        match self {
            AutoSpawnError::Timeout { .. } => crate::exit::DAEMON_UNREACHABLE,
            _ => crate::exit::GENERAL,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutoSpawnConfig {
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub binary: PathBuf,
    pub log_path: PathBuf,
    /// Total readiness budget. Default 5 s per FR2.3.
    pub budget: Duration,
    /// Initial backoff between socket existence checks. Default 10 ms.
    pub initial_backoff: Duration,
    /// Cap on backoff. Default 640 ms.
    pub max_backoff: Duration,
}

impl AutoSpawnConfig {
    pub fn defaults(socket: PathBuf, lock: PathBuf, binary: PathBuf, log_path: PathBuf) -> Self {
        Self {
            socket,
            lock,
            binary,
            log_path,
            budget: Duration::from_secs(5),
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(640),
        }
    }
}

/// Ensure the daemon is running. Returns when the socket exists and is ready
/// for the caller to connect.
pub async fn ensure_running(cfg: AutoSpawnConfig) -> Result<(), AutoSpawnError> {
    if cfg.socket.exists() {
        return Ok(());
    }

    if let Some(parent) = cfg.lock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&cfg.lock)
        .map_err(|e| AutoSpawnError::Lock(cfg.lock.clone(), e))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| AutoSpawnError::Lock(cfg.lock.clone(), e))?;

    if cfg.socket.exists() {
        let _ = FileExt::unlock(&lock_file);
        return Ok(());
    }

    spawn_detached(&cfg.binary)
        .map_err(|e| AutoSpawnError::Spawn(cfg.binary.clone(), e))?;

    let ready = poll_socket(
        &cfg.socket,
        cfg.budget,
        cfg.initial_backoff,
        cfg.max_backoff,
    )
    .await;

    let _ = FileExt::unlock(&lock_file);

    if ready {
        Ok(())
    } else {
        Err(AutoSpawnError::Timeout {
            socket: cfg.socket,
            budget: cfg.budget,
            log: cfg.log_path,
        })
    }
}

/// Poll for `socket` to exist on disk, with exponential backoff capped at
/// `max_backoff` and a hard deadline of `budget`. Returns true if the socket
/// appears within the budget.
pub async fn poll_socket(
    socket: &Path,
    budget: Duration,
    initial_backoff: Duration,
    max_backoff: Duration,
) -> bool {
    let deadline = Instant::now() + budget;
    let mut backoff = initial_backoff;
    if socket.exists() {
        return true;
    }
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let sleep_for = backoff.min(remaining);
        if sleep_for.is_zero() {
            break;
        }
        tokio::time::sleep(sleep_for).await;
        if socket.exists() {
            return true;
        }
        backoff = (backoff * 2).min(max_backoff);
    }
    socket.exists()
}

/// Spawn the daemon as a detached child. The child is placed in its own
/// session via `setsid(2)` so SIGHUP from the parent shell does not propagate.
///
/// The child runs `daemon start --foreground` so it actually executes the
/// daemon main loop in-process. Without `--foreground` the child would itself
/// re-enter the auto-spawn path and never bind the socket.
fn spawn_detached(binary: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(binary);
    cmd.arg("daemon")
        .arg("start")
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and only mutates session/process-
    // group state. No allocations or non-AS-safe calls in this closure.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(|errno| std::io::Error::from_raw_os_error(errno as i32))
        });
    }
    match cmd.spawn() {
        Ok(_child) => Ok(()),
        Err(e) => {
            warn!(error = %e, "auto-spawn: failed to launch daemon");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn poll_succeeds_when_socket_appears() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        let sock_clone = sock.clone();

        // Producer task: create the socket file after a short delay so the
        // poller has to wait at least one backoff cycle.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            fs::File::create(&sock_clone).unwrap();
        });

        let appeared = poll_socket(
            &sock,
            Duration::from_secs(2),
            Duration::from_millis(10),
            Duration::from_millis(100),
        )
        .await;
        assert!(appeared, "socket should have appeared within 2s");
    }

    #[tokio::test]
    async fn poll_returns_false_after_budget() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("never.sock");

        let appeared = poll_socket(
            &sock,
            Duration::from_millis(100),
            Duration::from_millis(10),
            Duration::from_millis(40),
        )
        .await;
        assert!(!appeared, "socket should never appear");
    }

    #[tokio::test]
    async fn poll_returns_immediately_when_socket_already_exists() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        fs::File::create(&sock).unwrap();
        let started = Instant::now();
        let appeared = poll_socket(
            &sock,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_millis(640),
        )
        .await;
        assert!(appeared);
        assert!(started.elapsed() < Duration::from_millis(50), "should not have slept");
    }

    #[test]
    fn timeout_maps_to_exit_4() {
        let err = AutoSpawnError::Timeout {
            socket: PathBuf::from("/tmp/x.sock"),
            budget: Duration::from_secs(5),
            log: PathBuf::from("/tmp/daemon.log"),
        };
        assert_eq!(err.exit_code(), 4);
    }
}
