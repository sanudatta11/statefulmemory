//! Daemon auto-spawn + liveness probe: shared by CLI, MCP, and hooks.
//!
//! Spec sections: FR2, PRD §4.1, EH-7, SC-1, SC-2.
//!
//! Algorithm:
//! 1. If the socket file exists, [`probe`] with a real `DaemonStatus` RPC
//!    (a file-only check cannot detect a crash that left the socket behind).
//! 2. Healthy → [`EnsureOutcome::AlreadyRunning`].
//! 3. Missing or stale → acquire `flock` on `daemon.lock`, re-check, remove
//!    a stale socket, spawn `daemon start --foreground` detached, poll for
//!    readiness (5 s budget, 10 ms → 640 ms backoff).
//! 4. On timeout return [`AutoSpawnError::Timeout`] (CLI exit 4).

use std::fs::File;
use std::future::Future;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use thiserror::Error;
use tracing::warn;

use crate::channel::connect_uds;
use crate::StatefulMemoryClient;

/// FR13 exit codes (kept in sync with `statefulmemory-cli::exit`).
pub const EXIT_GENERAL: u8 = 1;
pub const EXIT_DAEMON_UNREACHABLE: u8 = 4;

#[derive(Debug, Error)]
pub enum AutoSpawnError {
    #[error(
        "daemon not reachable: socket never appeared at {socket} within {budget:?}; check {log}"
    )]
    Timeout {
        socket: PathBuf,
        budget: Duration,
        log: PathBuf,
    },

    #[error("could not acquire spawn lock at {0}: {1}")]
    Lock(PathBuf, std::io::Error),

    #[error("could not spawn daemon at {0}: {1}")]
    Spawn(PathBuf, std::io::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl AutoSpawnError {
    /// FR13: timeout → 4 (daemon unreachable); other failures → 1.
    pub fn exit_code(&self) -> u8 {
        match self {
            AutoSpawnError::Timeout { .. } => EXIT_DAEMON_UNREACHABLE,
            _ => EXIT_GENERAL,
        }
    }
}

/// What [`ensure_running`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// Socket present and answered `DaemonStatus` — no spawn needed.
    AlreadyRunning,
    /// A detached daemon child was spawned (or a racer's spawn was adopted).
    Spawned {
        /// True when a stale socket file was cleared before spawn.
        was_stale: bool,
    },
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

/// Probe daemon liveness with a bounded `DaemonStatus` RPC. Returns false on
/// missing socket, connect error, RPC error, or timeout.
pub async fn probe(socket: &Path) -> bool {
    if !socket.exists() {
        return false;
    }
    let channel = match connect_uds(socket) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut client = StatefulMemoryClient::new(channel);
    matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            client.daemon_status(statefulmemory_proto::DaemonStatusRequest {}),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Ensure the daemon is running. Probes liveness when a socket file exists
/// and repairs a stale socket (crash leftover) by removing it and respawning.
///
/// Returns when the socket exists and (on the `AlreadyRunning` path) has
/// answered a status RPC.
pub async fn ensure_running(cfg: &AutoSpawnConfig) -> Result<EnsureOutcome, AutoSpawnError> {
    let mut was_stale = false;

    if cfg.socket.exists() {
        if probe(&cfg.socket).await {
            return Ok(EnsureOutcome::AlreadyRunning);
        }
        was_stale = true;
        remove_stale_socket(&cfg.socket)?;
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

    // Re-check under the lock: a racer may have spawned a healthy daemon.
    if cfg.socket.exists() {
        if probe(&cfg.socket).await {
            let _ = FileExt::unlock(&lock_file);
            return Ok(EnsureOutcome::AlreadyRunning);
        }
        was_stale = true;
        remove_stale_socket(&cfg.socket)?;
    }

    spawn_detached(&cfg.binary).map_err(|e| AutoSpawnError::Spawn(cfg.binary.clone(), e))?;

    // CRITICAL: release the spawn lock BEFORE polling. The lock's purpose is
    // to serialize concurrent auto-spawners — once the daemon child has been
    // forked, our work is done and any racing CLI should observe either the
    // child's lock or its socket. The daemon child opens the same lock file
    // and calls `try_lock_exclusive` (lifecycle::acquire_lock_and_pid); if we
    // were still holding our exclusive lock, the daemon would fail to start,
    // poll_socket would time out, and the user would see exit 4 even though
    // everything else was healthy.
    let _ = FileExt::unlock(&lock_file);
    drop(lock_file);

    let ready = poll_socket(
        &cfg.socket,
        cfg.budget,
        cfg.initial_backoff,
        cfg.max_backoff,
    )
    .await;

    if ready {
        Ok(EnsureOutcome::Spawned { was_stale })
    } else {
        Err(AutoSpawnError::Timeout {
            socket: cfg.socket.clone(),
            budget: cfg.budget,
            log: cfg.log_path.clone(),
        })
    }
}

/// Best-effort unlink of a stale socket. Refuses to remove a file we do not
/// own (same rule as daemon `unlink_stale_socket`).
fn remove_stale_socket(path: &Path) -> Result<(), AutoSpawnError> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(md) => {
            let our_uid = nix::unistd::getuid().as_raw();
            if md.uid() != our_uid {
                // Leave it; spawn path will likely fail lock/bind with a clear error.
                return Ok(());
            }
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(AutoSpawnError::Io(e)),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AutoSpawnError::Io(e)),
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
    // Explicitly forward data-dir / model env: some test harnesses start the
    // CLI with a cleared environment, and we must not drop these when the
    // child is reaped into its own session.
    for key in [
        "STATEFULMEMORY_DATA_DIR",
        "STATEFULMEMORY_LOG",
        "STATEFULMEMORY_BGE_MODEL_DIR",
        "HOME",
        "TMPDIR",
        "PATH",
    ] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
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

/// Marker used by callers that only need a fire-and-forget recovery future.
/// Kept object-safe for MCP `Recovery` implementors without `async-trait`.
pub type RecoverFuture<'a> = std::pin::Pin<Box<dyn Future<Output = Result<EnsureOutcome, AutoSpawnError>> + Send + 'a>>;

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
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "should not have slept"
        );
    }

    #[test]
    fn timeout_maps_to_exit_4() {
        let err = AutoSpawnError::Timeout {
            socket: PathBuf::from("/tmp/x.sock"),
            budget: Duration::from_secs(5),
            log: PathBuf::from("/tmp/daemon.log"),
        };
        assert_eq!(err.exit_code(), EXIT_DAEMON_UNREACHABLE);
        assert_eq!(err.exit_code(), 4);
    }

    #[tokio::test]
    async fn probe_false_when_socket_missing() {
        let dir = TempDir::new().unwrap();
        assert!(!probe(&dir.path().join("nope.sock")).await);
    }

    #[tokio::test]
    async fn probe_false_when_socket_is_dead_file() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        fs::File::create(&sock).unwrap();
        assert!(!probe(&sock).await, "regular file must not pass probe");
    }

    /// Fake daemon binary: creates the socket path baked into the script, then exits.
    fn write_fake_binary(dir: &TempDir, sock: &Path) -> PathBuf {
        let bin = dir.path().join("fake-daemon.sh");
        let script = format!("#!/bin/sh\ntouch '{}'\n", sock.display());
        fs::write(&bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    fn fast_cfg(dir: &TempDir, sock: PathBuf, bin: PathBuf) -> AutoSpawnConfig {
        let mut cfg = AutoSpawnConfig::defaults(
            sock,
            dir.path().join("daemon.lock"),
            bin,
            dir.path().join("daemon.log"),
        );
        cfg.budget = Duration::from_secs(2);
        cfg.initial_backoff = Duration::from_millis(10);
        cfg.max_backoff = Duration::from_millis(50);
        cfg
    }

    #[tokio::test]
    async fn ensure_spawns_when_socket_missing() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        let bin = write_fake_binary(&dir, &sock);
        let cfg = fast_cfg(&dir, sock.clone(), bin);

        let out = ensure_running(&cfg).await.expect("spawn");
        assert_eq!(
            out,
            EnsureOutcome::Spawned { was_stale: false },
            "missing socket should spawn fresh"
        );
        assert!(sock.exists());
    }

    #[tokio::test]
    async fn ensure_repairs_stale_socket() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        // Crash leftover: regular file, no listener.
        fs::File::create(&sock).unwrap();
        let bin = write_fake_binary(&dir, &sock);
        let cfg = fast_cfg(&dir, sock.clone(), bin);

        let out = ensure_running(&cfg).await.expect("repair spawn");
        assert_eq!(
            out,
            EnsureOutcome::Spawned { was_stale: true },
            "dead socket file must be treated as stale"
        );
        assert!(sock.exists());
    }

    #[tokio::test]
    async fn ensure_timeout_when_binary_never_creates_socket() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("daemon.sock");
        let bin = dir.path().join("noop.sh");
        fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = fast_cfg(&dir, sock.clone(), bin);
        cfg.budget = Duration::from_millis(200);

        let err = ensure_running(&cfg).await.expect_err("must time out");
        assert!(matches!(err, AutoSpawnError::Timeout { .. }), "{err:?}");
        assert_eq!(err.exit_code(), 4);
    }
}
