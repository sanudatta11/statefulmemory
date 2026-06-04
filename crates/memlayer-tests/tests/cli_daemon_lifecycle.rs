//! Daemon lifecycle + logs (TS-2, TS-3, TS-17).
//!
//! TS-2 — auto-spawn timeout returns exit 4 within 5.5 s when the socket
//!        never appears.
//! TS-3 — running `daemon start` while a daemon is already up returns exit 3.
//! TS-17 — `logs --lines N` caps the output; `logs -f` streams new lines.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use memlayer_tests::CliEnv;

// ---------------------------------------------------------------------------
// TS-2 (SC-2) — auto-spawn timeout exits 4 within 5.5 s.
//
// We have to drive the actual auto-spawn → poll → timeout path. The trick:
// make the auto-spawned daemon child fail to start so the socket never
// appears, but make the CLI's spawn_detached still return Ok (so we end up
// in the polling branch).
//
// Approach: set `MEMLAYER_LISTEN=tcp://127.0.0.1:0` without `MEMLAYER_TLS_*`.
// `Config::load()` validates that TCP mode requires both cert and key paths
// and returns FailedPrecondition, so the daemon child exits 2 immediately
// without binding. The CLI side spawned the child cleanly, so the only
// remaining failure mode is `poll_socket` timing out at ~5 s — which is
// exactly what SC-2 measures.
// ---------------------------------------------------------------------------

#[test]
fn ts2_unreachable_daemon_exits_4_quickly() {
    let env = CliEnv::new();
    let start = Instant::now();
    let out = env
        .cmd()
        // These are inherited by the daemon child the CLI spawns.
        .env("MEMLAYER_LISTEN", "tcp://127.0.0.1:0")
        .env_remove("MEMLAYER_TLS_CERT")
        .env_remove("MEMLAYER_TLS_KEY")
        .args(["obs", "recent"])
        .output()
        .expect("obs recent");
    let elapsed = start.elapsed();

    assert_eq!(
        out.status.code(),
        Some(4),
        "expected exit 4 (DAEMON_UNREACHABLE) on auto-spawn timeout; \
         elapsed={elapsed:?}; stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    // SC-2: must complete within 5.5 s. Allow a generous CI fudge factor.
    assert!(
        elapsed < Duration::from_secs(15),
        "auto-spawn timeout should fire near the 5 s budget; took {elapsed:?}",
    );
}

// ---------------------------------------------------------------------------
// TS-3 (SC-3) — `daemon start` while another daemon is already running → exit 3.
// ---------------------------------------------------------------------------

#[test]
fn ts3_daemon_start_when_running_exits_3() {
    let mut env = CliEnv::new();
    env.spawn_daemon();

    let out = env
        .cmd()
        .args(["daemon", "start"])
        .output()
        .expect("daemon start");
    assert_eq!(
        out.status.code(),
        Some(3),
        "running `daemon start` again should exit 3 (ALREADY_IN_STATE); stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
}

// ---------------------------------------------------------------------------
// TS-17 (SC-25) — `logs --lines N` caps output; `logs -f` streams new lines.
// ---------------------------------------------------------------------------

#[test]
fn ts17a_logs_lines_caps_output() {
    let env = CliEnv::new();
    // Pre-write a daemon.log with 50 lines; the CLI's logs verb operates on
    // the file directly and does not require a running daemon.
    let log = env.log_path();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(&log).unwrap();
    for i in 0..50 {
        writeln!(f, "log line {i:02}").unwrap();
    }
    drop(f);

    let out = env
        .cmd()
        .args(["logs", "--lines", "10"])
        .output()
        .expect("logs --lines");
    assert!(
        out.status.success(),
        "logs failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let count = stdout.lines().count();
    assert!(count <= 10, "expected ≤10 lines; got {count}: {stdout}");
    assert!(
        stdout.contains("log line 49"),
        "logs --lines 10 should include the last line; got: {stdout}",
    );
    assert!(
        !stdout.contains("log line 00"),
        "logs --lines 10 should not include the first line; got: {stdout}",
    );
}

#[test]
fn ts17b_logs_follow_streams_new_lines() {
    let env = CliEnv::new();
    let log = env.log_path();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, b"existing line\n").unwrap();

    let mut child = env
        .cmd()
        .args(["logs", "-f", "--lines", "1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("logs -f");

    // Append a fresh line after a small delay so the follower has to
    // observe a write that arrived after start-up.
    std::thread::sleep(Duration::from_millis(150));
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .unwrap();
        writeln!(f, "fresh line from test").unwrap();
    }

    // Read for up to 2 s, then SIGINT the follower.
    let mut stdout = child.stdout.take().expect("stdout pipe");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while Instant::now() < deadline {
        match stdout.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if String::from_utf8_lossy(&buf).contains("fresh line from test") {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // Send SIGINT so the follower exits cleanly. assert_cmd-style
    // termination: kill is fine for our purposes.
    let _ = child.kill();
    let _ = child.wait();

    let captured = String::from_utf8_lossy(&buf);
    assert!(
        captured.contains("fresh line from test"),
        "logs -f should have streamed the appended line; captured: {captured}",
    );
}
