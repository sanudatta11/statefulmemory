//! `memlayer` binary entry point.
//!
//! Spec 1 ships a minimal subcommand surface: `daemon start [--foreground]`
//! and `daemon stop`. The full clap CLI (obs/session/prompt/team/etc.) lives
//! in Spec 2's `memlayer-cli` crate, which depends on this binary's daemon
//! library for the gRPC service definitions and re-exports a different `main`.
//!
//! Until Spec 2 is wired in, this binary suffices for integration tests:
//! ```bash
//! memlayer daemon start --foreground
//! ```

use std::process::ExitCode;

use tracing::error;

use memlayer_core::config::Config;
use memlayer_daemon::server;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
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
            println!("memlayer {} (commit unknown, built unknown)", env!("CARGO_PKG_VERSION"));
            ExitCode::from(0)
        }
        Mode::UsageError(msg) => {
            eprintln!("usage error: {msg}\n\nUSAGE:\n  memlayer daemon start [--foreground]\n  memlayer daemon stop\n  memlayer version");
            ExitCode::from(2)
        }
    }
}

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
    match (args[0].as_str(), args.get(1).map(|s| s.as_str()), args.get(2).map(|s| s.as_str())) {
        ("daemon", Some("start"), opt) => Mode::DaemonStart {
            foreground: opt == Some("--foreground"),
        },
        ("daemon", Some("stop"), _) => Mode::DaemonStop,
        ("version", _, _) => Mode::Version,
        (other, _, _) => Mode::UsageError(format!("unknown command '{other}'")),
    }
}

fn read_pid_and_signal() -> std::result::Result<(), u8> {
    let pid_path = memlayer_core::paths::pid_path();
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
