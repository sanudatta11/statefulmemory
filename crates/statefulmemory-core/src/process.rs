//! Bounded child-process helpers.
//!
//! No caller may block forever on a child: every wait has a hard deadline,
//! and the child is SIGKILLed when the deadline fires. Used by the install
//! path (pip / venv / python probes) and by integration tests so a hung
//! process cannot eat CI or a developer machine indefinitely.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

/// Default bound for short CLI probes (version checks, local tools).
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(5);
/// Default bound for test harness CLI invocations.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Spawn `cmd` with piped stdout/stderr, wait at most `timeout`, kill on expiry.
///
/// On timeout returns [`io::ErrorKind::TimedOut`]; the child is already dead.
pub fn output_with_timeout(cmd: &mut Command, timeout: Duration) -> io::Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn()?;
    wait_child(child, timeout)
}

/// Spawn `cmd` with inherited stdio, wait at most `timeout`, kill on expiry.
pub fn status_with_timeout(cmd: &mut Command, timeout: Duration) -> io::Result<ExitStatus> {
    let mut child = cmd.spawn()?;
    wait_status(&mut child, timeout)
}

/// Wait on an already-spawned child (piped stdout/stderr) with a hard deadline.
///
/// Drains pipes on background threads so a chatty child cannot deadlock on a
/// full pipe buffer while the deadline loop runs.
pub fn wait_child(mut child: Child, timeout: Duration) -> io::Result<Output> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let t_out = stdout.map(drain_on_thread);
    let t_err = stderr.map(drain_on_thread);

    let status = wait_status(&mut child, timeout)?;
    let stdout = join_drain(t_out);
    let stderr = join_drain(t_err);
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn drain_on_thread<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

fn join_drain(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default()
}

/// Poll `child.try_wait()` until exit or `timeout`; kill + reap on expiry.
fn wait_status(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            // Reap so we never leave a zombie behind the kill.
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("child process exceeded {timeout:?} and was killed"),
            ));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_within_budget() {
        let mut cmd = Command::new("echo");
        cmd.arg("hi");
        let out = output_with_timeout(&mut cmd, Duration::from_secs(5)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }

    #[test]
    fn hangs_are_killed_with_timed_out() {
        // `sleep 60` would eat a minute of CI if unbounded.
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let start = Instant::now();
        let err = output_with_timeout(&mut cmd, Duration::from_millis(200)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "kill must fire near the deadline, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn status_with_timeout_kills_hangs() {
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let err = status_with_timeout(&mut cmd, Duration::from_millis(200)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn large_output_does_not_deadlock() {
        // >64 KiB pipe buffer: drain threads must keep the child unblocked.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes x | head -c 200000"]);
        let out = output_with_timeout(&mut cmd, Duration::from_secs(10)).unwrap();
        assert!(out.stdout.len() >= 200_000, "got {}", out.stdout.len());
    }
}
