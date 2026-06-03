//! `memlayer` binary entry point.
//!
//! Parses [`cli::Cli`], dispatches to per-noun command modules. As of
//! spec2-t7 every group is wired: obs/session/prompt/project/sync/daemon/
//! team/logs/version. `open_client` now auto-spawns the daemon (FR2) when
//! the socket is missing.

use std::process::ExitCode;

use clap::Parser;
use is_terminal::IsTerminal;
use tracing::error;

use memlayer_cli::cli::{Cli, Command, DaemonArgs, DaemonVerb, OutputFormat};
use memlayer_cli::{
    cmd_daemon, cmd_logs, cmd_obs, cmd_project, cmd_prompt, cmd_session, cmd_sync, cmd_team,
    cmd_version,
};
use memlayer_cli::{autospawn, exit};
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
        // daemon start --foreground runs the daemon in this process; never
        // tries to connect to itself, so it sits outside open_client.
        Command::Daemon(DaemonArgs { verb: DaemonVerb::Start { foreground: true } }) => {
            run_daemon_foreground().await
        }
        Command::Daemon(DaemonArgs { verb }) => {
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            cmd_daemon::dispatch(fmt, verb).await
        }
        Command::Version => cmd_version::run(),
        Command::Logs(args) => cmd_logs::dispatch(args).await,
        Command::Team(args) => {
            // Token verbs need a client; init-ca doesn't. Open the client
            // optimistically, but tolerate the daemon being absent for
            // init-ca.
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            let needs_client = !matches!(args.verb, memlayer_cli::cli::TeamVerb::InitCa(_));
            if needs_client {
                match open_client(cli.output, cli.project).await {
                    Ok((mut client, _project, fmt)) => {
                        cmd_team::dispatch(Some(&mut client), fmt, args.verb).await
                    }
                    Err(code) => code,
                }
            } else {
                cmd_team::dispatch(None, fmt, args.verb).await
            }
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

/// Resolve the project, open a UDS gRPC client (auto-spawning the daemon
/// per FR2 if the socket is missing), pick the formatter, and hand the
/// prepared trio back. Returns `Err(exit_code)` if any step fails.
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

    // Auto-spawn (FR2): if the socket is missing, try to spawn the daemon
    // before issuing any RPC. ensure_running takes the flock, double-checks
    // the socket, then spawns + polls.
    if !socket.exists() {
        let cfg = cmd_daemon::default_autospawn_config();
        if let Err(e) = autospawn::ensure_running(cfg).await {
            eprintln!("memlayer: {e}");
            return Err(ExitCode::from(e.exit_code()));
        }
    }

    let channel = match client_channel::connect_uds(&socket) {
        Ok(c) => c,
        Err(ClientError::SocketNotFound(_)) => {
            eprintln!(
                "memlayer: daemon socket not found at {} after auto-spawn; check {}",
                socket.display(),
                memlayer_core::paths::log_path().display()
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
