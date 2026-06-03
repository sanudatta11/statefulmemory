//! TS-22: graceful shutdown via SIGTERM.

use std::time::{Duration, Instant};

use memlayer_tests::spawn_daemon;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

#[tokio::test]
async fn ts22_sigterm_graceful_exit() {
    let mut h = spawn_daemon();
    // Wait for the socket to appear so we know the daemon is up.
    let sock = h.data_dir.path().join("daemon.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sock.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sock.exists(), "socket never appeared");

    let pid = h.child.id();
    kill(Pid::from_raw(pid as i32), Signal::SIGTERM).expect("SIGTERM");

    // Daemon should exit within ~6 s (5s drain + slack).
    let exit_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match h.child.try_wait().unwrap() {
            Some(status) => {
                assert!(
                    status.success() || status.code() == Some(0),
                    "expected clean exit; got {status:?}"
                );
                return;
            }
            None => {
                if Instant::now() > exit_deadline {
                    let _ = h.child.kill();
                    panic!("daemon did not exit within drain window");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
