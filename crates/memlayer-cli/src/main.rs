//! `memlayer` binary entry point.
//!
//! Parses [`cli::Cli`], dispatches to per-noun command modules. As of
//! spec2-t5 the `obs` group is fully wired; `daemon start --foreground` and
//! `version` are also live. Sessions, prompts, project, team, logs, sync
//! still print "not yet implemented" stubs and ship in spec2-t6/t7.

use std::process::ExitCode;

use clap::Parser;
use is_terminal::IsTerminal;
use tracing::error;

use memlayer_cli::cli::{Cli, Command, DaemonArgs, DaemonVerb};
use memlayer_cli::cmd_obs;
use memlayer_cli::exit;
use memlayer_cli::formatter::Formatter;
use memlayer_cli::project_detect;
use memlayer_client::{channel as client_channel, ClientError, MemlayerClient};

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
            println!(
                "memlayer {} (commit unknown, built unknown)",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::SUCCESS
        }
        Command::Obs(args) => run_obs(cli.output, cli.project, cli.no_color, cli.quiet, args).await,
        other => {
            // The full surface lights up in spec2-t6..t7.
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

/// Resolve the project, open the gRPC client over UDS, and dispatch to
/// `cmd_obs`. Errors map to FR13 exit codes.
async fn run_obs(
    output: Option<memlayer_cli::cli::OutputFormat>,
    project_flag: Option<String>,
    _no_color: bool,
    quiet: bool,
    args: memlayer_cli::cli::ObsArgs,
) -> ExitCode {
    // 1. Project detection (FR3, PRD §9.1). The clap-level --project /
    //    MEMLAYER_PROJECT capture is in `Cli`; treat both as the same
    //    cli-override input here. Empty string means "not provided" (clap
    //    leaves the env-pulled value as Some("") when MEMLAYER_PROJECT="").
    let cli_override = project_flag.filter(|s| !s.is_empty());
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("memlayer: getcwd: {e}");
            return ExitCode::from(exit::GENERAL);
        }
    };
    let detection = match project_detect::detect_in(&cwd, None, cli_override) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("memlayer: {e}");
            return ExitCode::from(e.exit_code());
        }
    };
    if !quiet {
        // Stay quiet on stderr by default — agents pipe a lot of these and
        // every line of stderr ends up in their context budget.
    }

    // 2. Open the gRPC channel over UDS.
    let socket = memlayer_core::paths::socket_path();
    let channel = match client_channel::connect_uds(&socket) {
        Ok(c) => c,
        Err(ClientError::SocketNotFound(_)) => {
            // Auto-spawn lands in spec2-t7 (depends on cmd_daemon::start).
            // Until then, surface a clear error pointing the user at
            // `memlayer daemon start`.
            eprintln!(
                "memlayer: daemon socket not found at {}; run `memlayer daemon start --foreground` first",
                socket.display()
            );
            return ExitCode::from(exit::DAEMON_UNREACHABLE);
        }
        Err(e) => {
            eprintln!("memlayer: {e}");
            return ExitCode::from(exit::GENERAL);
        }
    };
    let mut client = MemlayerClient::new(channel);

    // 3. Pick the formatter (FR1.2, SC-4, SC-5).
    let stdout_is_tty = std::io::stdout().is_terminal();
    let fmt = Formatter::resolve(output.map(|f| f.to_formatter()), stdout_is_tty);

    cmd_obs::dispatch(&mut client, &detection.normalized, fmt, quiet, args.verb).await
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
