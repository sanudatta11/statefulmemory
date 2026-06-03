//! `memlayer` binary entry point.
//!
//! Parses [`cli::Cli`], dispatches to per-noun command modules. As of
//! spec2-t6 the `obs`, `session`, `prompt`, `project`, and `sync` groups
//! are wired; `daemon start --foreground` and `version` are also live.
//! `daemon` (non-foreground), `team`, and `logs` ship in spec2-t7.

use std::process::ExitCode;

use clap::Parser;
use is_terminal::IsTerminal;
use tracing::error;

use memlayer_cli::cli::{Cli, Command, DaemonArgs, DaemonVerb, OutputFormat};
use memlayer_cli::{cmd_obs, cmd_project, cmd_prompt, cmd_session, cmd_sync};
use memlayer_cli::exit;
use memlayer_cli::formatter::Formatter;
use memlayer_cli::project_detect;
use memlayer_client::{channel as client_channel, ClientError, MemlayerClient};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => e.exit(),
    };

    match cli.command {
        Command::Daemon(DaemonArgs { verb: DaemonVerb::Start { foreground: true } }) => {
            run_daemon_foreground().await
        }
        Command::Version => {
            println!(
                "memlayer {} (commit unknown, built unknown)",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::SUCCESS
        }
        Command::Obs(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, project, fmt)) => {
                cmd_obs::dispatch(&mut client, &project, fmt, cli.quiet, args.verb).await
            }
            Err(code) => code,
        },
        Command::Session(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, project, fmt)) => {
                cmd_session::dispatch(&mut client, &project, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Prompt(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, project, fmt)) => {
                cmd_prompt::dispatch(&mut client, &project, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Project(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, project, fmt)) => {
                cmd_project::dispatch(&mut client, &project, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Sync(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, project, fmt)) => {
                cmd_sync::dispatch(&mut client, &project, fmt, args.verb).await
            }
            Err(code) => code,
        },
        other => {
            // daemon (non-foreground), team, logs — spec2-t7.
            eprintln!(
                "memlayer: '{}' is not yet implemented in this build",
                verb_label(&other)
            );
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

/// Resolve the project, open a UDS gRPC client, pick the formatter, and
/// hand the prepared trio back to the caller. Returns `Err(exit_code)` if
/// any step fails (project detection, channel open).
async fn open_client(
    output: Option<OutputFormat>,
    project_flag: Option<String>,
) -> Result<(MemlayerClient<tonic::transport::Channel>, String, Formatter), ExitCode> {
    let cli_override = project_flag.filter(|s| !s.is_empty());
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("memlayer: getcwd: {e}");
            return Err(ExitCode::from(exit::GENERAL));
        }
    };
    let detection = match project_detect::detect_in(&cwd, None, cli_override) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("memlayer: {e}");
            return Err(ExitCode::from(e.exit_code()));
        }
    };
    let socket = memlayer_core::paths::socket_path();
    let channel = match client_channel::connect_uds(&socket) {
        Ok(c) => c,
        Err(ClientError::SocketNotFound(_)) => {
            eprintln!(
                "memlayer: daemon socket not found at {}; run `memlayer daemon start --foreground` first",
                socket.display()
            );
            return Err(ExitCode::from(exit::DAEMON_UNREACHABLE));
        }
        Err(e) => {
            eprintln!("memlayer: {e}");
            return Err(ExitCode::from(exit::GENERAL));
        }
    };
    let client = MemlayerClient::new(channel);
    let stdout_is_tty = std::io::stdout().is_terminal();
    let fmt = Formatter::resolve(output.map(|f| f.to_formatter()), stdout_is_tty);
    Ok((client, detection.normalized, fmt))
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
