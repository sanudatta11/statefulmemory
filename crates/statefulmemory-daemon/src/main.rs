//! `statefulmemory` binary entry point.
//!
//! Spec 1 ships a minimal subcommand surface: `daemon start [--foreground]`
//! and `daemon stop`. The full clap CLI (obs/session/prompt/team/etc.) lives
//! in Spec 2's `statefulmemory-cli` crate, which depends on this binary's daemon
//! library for the gRPC service definitions and re-exports a different `main`.
//!
//! Until Spec 2 is wired in, this binary suffices for integration tests:
//! ```bash
//! statefulmemory daemon start --foreground
//! ```

use std::process::ExitCode;
use std::time::Duration;

use tracing::error;

use statefulmemory_core::config::Config;
use statefulmemory_daemon::server;

fn main() -> ExitCode {
    // Manual runtime so shutdown can be hard-bounded: `#[tokio::main]`'s
    // implicit `Runtime::drop` joins the blocking pool indefinitely, which
    // means a stuck `spawn_blocking` (e.g. a stalled BGE model download)
    // keeps the daemon alive long after SIGTERM drain finished. Tests that
    // wait for exit would then hang for the full stall duration.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let code = rt.block_on(run());
    // 6 s: graceful drain is 5 s (signals spec) + 1 s slack for join.
    rt.shutdown_timeout(Duration::from_secs(6));
    code
}

async fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Mode::DaemonStart { foreground: _ } => match Config::load() {
            Ok(cfg) => match server::run(cfg).await {
                Ok(()) => ExitCode::from(0),
                Err(e) => {
                    error!(error=%e, "daemon exited with error");
                    ExitCode::from(1)
                }
            },
            Err(e) => {
                eprintln!("config error: {e}");
                ExitCode::from(2)
            }
        },
        Mode::DaemonStop => {
            // Stop is a thin client wrapper: connect to UDS, send Shutdown.
            // Full implementation is wired in Spec 2's CLI crate where the
            // gRPC client lives. For Spec 1, we approximate by sending SIGTERM
            // to the daemon's pidfile.
            match read_pid_and_signal() {
                Ok(()) => ExitCode::from(0),
                Err(code) => ExitCode::from(code),
            }
        }
        Mode::Version => {
            println!(
                "statefulmemory {} (commit unknown, built unknown)",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::from(0)
        }
        Mode::UsageError(msg) => {
            eprintln!("usage error: {msg}\n\nUSAGE:\n  statefulmemory daemon start [--foreground]\n  statefulmemory daemon stop\n  statefulmemory version");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
enum Mode {
    DaemonStart { foreground: bool },
    DaemonStop,
    Version,
    UsageError(String),
}

fn parse(args: &[String]) -> Mode {
    if args.is_empty() {
        return Mode::UsageError("no subcommand".into());
    }
    match (
        args[0].as_str(),
        args.get(1).map(|s| s.as_str()),
        args.get(2).map(|s| s.as_str()),
    ) {
        ("daemon", Some("start"), opt) => Mode::DaemonStart {
            foreground: opt == Some("--foreground"),
        },
        ("daemon", Some("stop"), _) => Mode::DaemonStop,
        ("version", _, _) => Mode::Version,
        (other, _, _) => Mode::UsageError(format!("unknown command '{other}'")),
    }
}

fn read_pid_and_signal() -> std::result::Result<(), u8> {
    let pid_path = statefulmemory_core::paths::pid_path();
    let body = std::fs::read_to_string(&pid_path).map_err(|e| {
        eprintln!("could not read pid file {}: {e}", pid_path.display());
        4u8
    })?;
    let pid: i32 = body.trim().parse().map_err(|_| {
        eprintln!("malformed pid file");
        1u8
    })?;
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid), Signal::SIGTERM).map_err(|e| {
        eprintln!("kill {pid}: {e}");
        1u8
    })?;
    Ok(())
}
