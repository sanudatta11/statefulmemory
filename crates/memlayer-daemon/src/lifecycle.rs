//! Daemon lifecycle: spawn lock, pidfile, FD-limit raise, graceful shutdown,
//! cleanup of socket/pid/lock on exit.
//!
//! Spec sections: FR1.1, FR1.5, FR2.7, FR2.8, FR10, SC-1, SC-2, SC-3.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use tracing::{info, warn};

use memlayer_core::error::{Error, Result};

/// Holds the resources that must be cleaned up on shutdown:
/// - the flock-owned `daemon.lock` file
/// - the `daemon.pid` file path (for unlink)
/// - the socket path (for unlink, UDS only)
pub struct LifecycleGuard {
    pub lock_file: File,
    pub pid_path: PathBuf,
    pub socket_path: Option<PathBuf>,
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        if let Some(sock) = &self.socket_path {
            let _ = std::fs::remove_file(sock);
        }
        let _ = std::fs::remove_file(&self.pid_path);
        let _ = self.lock_file.unlock();
    }
}

/// Acquire the daemon spawn lock and write the pidfile.
///
/// Returns `Error::AlreadyExists` if another daemon already holds the lock.
pub fn acquire_lock_and_pid(lock_path: &Path, pid_path: &Path) -> Result<File> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock_exclusive().map_err(|e| {
        Error::AlreadyExists(format!(
            "daemon already running (lock: {} held: {e})",
            lock_path.display()
        ))
    })?;
    let mut pidf = File::create(pid_path)?;
    writeln!(pidf, "{}", std::process::id())?;
    Ok(lock)
}

/// Best-effort raise of `RLIMIT_NOFILE` to 8192 (FR1.5, EC-17).
pub fn raise_fd_limit() {
    use nix::sys::resource::{setrlimit, Resource};
    let target = 8192;
    if let Err(e) = setrlimit(Resource::RLIMIT_NOFILE, target, target) {
        warn!(error=%e, "could not raise RLIMIT_NOFILE; continuing");
    } else {
        info!(target, "raised RLIMIT_NOFILE");
    }
}

/// Unlink stale socket file before binding (SC-2, FR2.7).
pub fn unlink_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    // Refuse to overwrite a file we don't own (EC-13).
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path)?;
    let our_uid = nix::unistd::getuid().as_raw();
    if md.uid() != our_uid {
        return Err(Error::FailedPrecondition(format!(
            "socket {} owned by uid {}, not us ({})",
            path.display(),
            md.uid(),
            our_uid
        )));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

/// Set socket file mode `0600` after binding (FR2.1).
pub fn chmod_socket_0600(path: &Path) -> Result<()> {
    let perm = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perm)?;
    Ok(())
}
