//! `memlayer` binary entry point.
//!
//! Parses [`cli::Cli`], dispatches to per-noun command modules. For Spec 2
//! task 2, `daemon start --foreground` and `version` are wired; every other
//! verb prints a "not yet implemented" stub. Tasks spec2-t5..t7 fill in the
//! rest.

use std::process::ExitCode;

use clap::Parser;
use tracing::error;

use memlayer_cli::cli::{Cli, Command, DaemonArgs, DaemonVerb};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // clap returns an Err for `--help` / `--version` too; those exit
            // with code 0 via `e.exit()` after writing to stdout.
            e.exit();
        }
    };

    match cli.command {
        Command::Daemon(DaemonArgs { verb: DaemonVerb::Start { foreground: true } }) => {
            run_daemon_foreground().await
        }
        Command::Version => {
            // Full version string with commit + build date arrives in
            // spec2-t7 via vergen; ship the basic crate version for now.
            println!(
                "memlayer {} (commit unknown, built unknown)",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::SUCCESS
        }
        other => {
            // The full surface lights up in spec2-t5..t7.
            eprintln!("memlayer: '{}' is not yet implemented in this build", verb_label(&other));
            ExitCode::from(1)
        }
    }
}

async fn run_daemon_foreground() -> ExitCode {
    use memlayer_core::config::Config;
    use memlayer_daemon::server;

    match Config::load() {
        Ok(cfg) => match server::run(cfg).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                error!(error = %e, "daemon exited with error");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("config error: {e}");
            ExitCode::from(2)
        }
    }
}

fn verb_label(cmd: &Command) -> &'static str {
    match cmd {
        Command::Obs(_) => "obs",
        Command::Session(_) => "session",
        Command::Prompt(_) => "prompt",
        Command::Project(_) => "project",
        Command::Sync(_) => "sync",
        Command::Daemon(_) => "daemon (non-foreground)",
        Command::Team(_) => "team",
        Command::Logs(_) => "logs",
        Command::Version => "version",
    }
}
