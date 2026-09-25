//! `statefulmemory` binary entry point.
//!
//! Parses [`cli::Cli`], dispatches to per-noun command modules. As of
//! spec2-t7 every group is wired: obs/session/prompt/project/sync/daemon/
//! team/logs/version. `open_client` now auto-spawns the daemon (FR2) when
//! the socket is missing.

use std::process::ExitCode;

use clap::Parser;
use is_terminal::IsTerminal;
use tracing::error;

use statefulmemory_cli::cli::{Cli, Command, DaemonArgs, DaemonVerb, HookVerb, OutputFormat};
use statefulmemory_cli::formatter::Formatter;
use statefulmemory_cli::project_detect::{self, ProjectDetection};
use statefulmemory_cli::{autospawn, exit};
use statefulmemory_cli::{
    cmd_context, cmd_daemon, cmd_decide, cmd_doctor, cmd_dream, cmd_eval, cmd_graph, cmd_hook,
    cmd_ingest, cmd_logs, cmd_mem, cmd_obs, cmd_project, cmd_prompt, cmd_session, cmd_skill,
    cmd_sync, cmd_team, cmd_tui, cmd_ui, cmd_uninstall, cmd_update, cmd_verify, cmd_version,
};
use statefulmemory_client::{channel as client_channel, ClientError, StatefulMemoryClient};
use statefulmemory_proto as p;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => e.exit(),
    };

    // FR1.4 / FR1.6: wire global flags before any dispatch. `--no-color` is
    // implemented as setting NO_COLOR in the environment, which composes with
    // users who already set the env var themselves; `--quiet` is read by the
    // info!() macro in the lib root.
    statefulmemory_cli::init_globals(cli.quiet, cli.no_color);
    if !cli.no_update_check && statefulmemory_cli::cmd_update::should_auto_update(&cli.command) {
        statefulmemory_cli::cmd_update::maybe_auto_update();
    }

    match cli.command {
        // daemon start --foreground runs the daemon in this process; never
        // tries to connect to itself, so it sits outside open_client.
        Command::Daemon(DaemonArgs {
            verb: DaemonVerb::Start { foreground: true },
        }) => run_daemon_foreground().await,
        Command::Daemon(DaemonArgs { verb }) => {
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            cmd_daemon::dispatch(fmt, verb).await
        }
        Command::Version => cmd_version::run(),
        Command::Update(args) => cmd_update::dispatch(args),
        Command::UpdateCheck => cmd_update::dispatch_check_helper(),
        Command::UpdateApply(args) => cmd_update::dispatch_apply_helper(args),
        Command::UpdateRollback(args) => cmd_update::dispatch_apply_helper(args),
        Command::Logs(args) => cmd_logs::dispatch(args).await,
        Command::Team(args) => {
            // Token verbs need a client; init-ca doesn't. Open the client
            // optimistically, but tolerate the daemon being absent for
            // init-ca.
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            let needs_client = !matches!(args.verb, statefulmemory_cli::cli::TeamVerb::InitCa(_));
            if needs_client {
                match open_client(cli.output, cli.project).await {
                    Ok((mut client, _detection, fmt)) => {
                        cmd_team::dispatch(Some(&mut client), fmt, args.verb).await
                    }
                    Err(code) => code,
                }
            } else {
                cmd_team::dispatch(None, fmt, args.verb).await
            }
        }
        Command::Obs(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_obs::dispatch(
                    &mut client,
                    &detection.normalized,
                    fmt,
                    cli.quiet,
                    args.verb,
                )
                .await
            }
            Err(code) => code,
        },
        Command::Session(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_session::dispatch(&mut client, &detection.normalized, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Prompt(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_prompt::dispatch(&mut client, &detection.normalized, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Project(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_project::dispatch(&mut client, &detection, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Install(args) => {
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            cmd_skill::dispatch(fmt, args).await
        }
        Command::Uninstall(args) => {
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            cmd_uninstall::dispatch_uninstall(fmt, args.purge).await
        }
        Command::Clean => {
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            cmd_uninstall::dispatch_clean(fmt).await
        }
        Command::Sync(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_sync::dispatch(&mut client, &detection.normalized, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Mem(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_mem::dispatch(&mut client, &detection.normalized, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Hook(args) => match args.verb {
            HookVerb::PreTool(pre) => match open_client(cli.output, cli.project).await {
                Ok((mut client, detection, _fmt)) => {
                    cmd_hook::dispatch(&mut client, &detection.normalized, pre).await
                }
                // Fail-silent: if the daemon isn't reachable, we MUST NOT
                // delay the agent's tool call. Exit 0 immediately.
                Err(_) => ExitCode::SUCCESS,
            },
            // SessionStart owns its own daemon repair (open_client won't clear
            // a stale socket), so detect the project directly and fail-silent
            // if detection is ambiguous.
            HookVerb::SessionStart(ss) => match detect_project_silent(cli.project) {
                Some(project) => cmd_hook::dispatch_session_start(&project, ss.limit).await,
                None => ExitCode::SUCCESS,
            },
        },
        Command::Config(args) => statefulmemory_cli::cmd_config::dispatch(args.verb).await,
        Command::Eval(args) => cmd_eval::dispatch(args, cli.output).await,
        Command::Decide(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_decide::dispatch(&mut client, &detection.normalized, fmt, args).await
            }
            Err(code) => code,
        },
        Command::Verify(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_verify::dispatch(&mut client, &detection.normalized, fmt, args).await
            }
            Err(code) => code,
        },
        Command::Graph(args) => {
            let stdout_is_tty = std::io::stdout().is_terminal();
            let fmt = Formatter::resolve(cli.output.map(|f| f.to_formatter()), stdout_is_tty);
            let needs_client = matches!(args.verb, statefulmemory_cli::cli::GraphVerb::Query(_));
            if needs_client {
                match open_client(cli.output, cli.project).await {
                    Ok((mut client, detection, fmt)) => {
                        cmd_graph::dispatch(
                            Some(&mut client),
                            &detection.normalized,
                            fmt,
                            args.verb,
                        )
                        .await
                    }
                    Err(code) => code,
                }
            } else {
                // Stats / rebuild read the project DB file directly, so they
                // only need project detection, never the daemon.
                let project = detect_project_silent(cli.project.clone())
                    .unwrap_or_else(|| "default".to_string());
                cmd_graph::dispatch(None, &project, fmt, args.verb).await
            }
        }
        Command::Context(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => match args.verb {
                statefulmemory_cli::cli::ContextVerb::Compile(a) => {
                    cmd_context::dispatch(&mut client, &detection.normalized, fmt, a).await
                }
            },
            Err(code) => code,
        },
        Command::Ingest(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => match args.verb {
                statefulmemory_cli::cli::IngestVerb::Repo(a) => {
                    cmd_ingest::dispatch(&mut client, &detection.normalized, fmt, a).await
                }
            },
            Err(code) => code,
        },
        Command::Dream(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, fmt)) => {
                cmd_dream::dispatch(&mut client, &detection.normalized, fmt, args.verb).await
            }
            Err(code) => code,
        },
        Command::Ui(args) => cmd_ui::dispatch(cli.project, &args).await,
        Command::Doctor(args) => {
            let project =
                detect_project_silent(cli.project.clone()).unwrap_or_else(|| "default".to_string());
            let is_json = matches!(
                cli.output,
                Some(statefulmemory_cli::cli::OutputFormat::Json)
            );
            match open_client(cli.output, cli.project).await {
                Ok((mut client, detection, _fmt)) => {
                    match cmd_doctor::run(Some(&mut client), &detection.normalized, args, is_json)
                        .await
                    {
                        Ok(_) => ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("statefulmemory doctor error: {e}");
                            ExitCode::FAILURE
                        }
                    }
                }
                Err(_) => match cmd_doctor::run(None, &project, args, is_json).await {
                    Ok(_) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("statefulmemory doctor error: {e}");
                        ExitCode::FAILURE
                    }
                },
            }
        }
        Command::Tui(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, _fmt)) => {
                match cmd_tui::run(&mut client, &detection.normalized, args.query).await {
                    Ok(_) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("statefulmemory tui error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            Err(code) => code,
        },
        Command::Mcp => statefulmemory_cli::cmd_mcp::dispatch(cli.project).await,
        Command::Reindex(args) => match open_client(cli.output, cli.project).await {
            Ok((mut client, detection, _fmt)) => {
                let req = p::ReindexObservationsRequest {
                    project_name: detection.normalized.clone(),
                    force: args.force,
                };
                match client.reindex_observations(req).await {
                    Ok(resp) => {
                        let r = resp.into_inner();
                        eprintln!(
                            "Queued {} observations for re-embedding, skipped {}, cleared {}.",
                            r.queued, r.skipped, r.cleared
                        );
                        ExitCode::SUCCESS
                    }
                    Err(s) => {
                        eprintln!("statefulmemory reindex: {}", s.message());
                        ExitCode::from(statefulmemory_cli::exit::from_status(s.code()))
                    }
                }
            }
            Err(code) => code,
        },
    }
}

async fn run_daemon_foreground() -> ExitCode {
    use statefulmemory_core::config::Config;
    use statefulmemory_daemon::server;

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

/// Resolve the project name without touching the daemon. Returns `None` if
/// the cwd is unreadable or project detection is ambiguous — callers in
/// fail-silent hook paths treat `None` as "skip".
fn detect_project_silent(project_flag: Option<String>) -> Option<String> {
    let cli_override = project_flag.filter(|s| !s.is_empty());
    let env_override = std::env::var("STATEFULMEMORY_PROJECT")
        .ok()
        .filter(|s| !s.is_empty());
    let cwd = std::env::current_dir().ok()?;
    project_detect::detect_in(&cwd, env_override, cli_override)
        .ok()
        .map(|d| d.normalized)
}

/// Resolve the project, open a UDS gRPC client (auto-spawning the daemon
/// per FR2 if the socket is missing), pick the formatter, and hand the
/// prepared trio back. Returns `Err(exit_code)` if any step fails.
///
/// Reads `STATEFULMEMORY_PROJECT` directly here (not via clap's `env=`) so that
/// `project_detect` can attribute the source as `env_override` rather than
/// `cli_flag` when only the env var is set. Precedence (highest first):
/// `--project` flag, `STATEFULMEMORY_PROJECT`, `.statefulmemory/config.json`, git
/// remote, git root basename.
async fn open_client(
    output: Option<OutputFormat>,
    project_flag: Option<String>,
) -> Result<
    (
        StatefulMemoryClient<tonic::transport::Channel>,
        ProjectDetection,
        Formatter,
    ),
    ExitCode,
> {
    let cli_override = project_flag.filter(|s| !s.is_empty());
    let env_override = std::env::var("STATEFULMEMORY_PROJECT")
        .ok()
        .filter(|s| !s.is_empty());
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("statefulmemory: getcwd: {e}");
            return Err(ExitCode::from(exit::GENERAL));
        }
    };
    let detection = match project_detect::detect_in(&cwd, env_override, cli_override) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("statefulmemory: {e}");
            return Err(ExitCode::from(e.exit_code()));
        }
    };
    let socket = statefulmemory_core::paths::socket_path();

    // Auto-spawn (FR2): always ensure — probe detects a stale socket left by
    // a crashed daemon, clears it, and respawns. Costs one local DaemonStatus
    // RPC when already healthy.
    let cfg = cmd_daemon::default_autospawn_config();
    if let Err(e) = autospawn::ensure_running(&cfg).await {
        eprintln!("statefulmemory: {e}");
        return Err(ExitCode::from(e.exit_code()));
    }

    let channel = match client_channel::connect_uds(&socket) {
        Ok(c) => c,
        Err(ClientError::SocketNotFound(_)) => {
            eprintln!(
                "statefulmemory: daemon socket not found at {} after auto-spawn; check {}",
                socket.display(),
                statefulmemory_core::paths::log_path().display()
            );
            return Err(ExitCode::from(exit::DAEMON_UNREACHABLE));
        }
        Err(e) => {
            eprintln!("statefulmemory: {e}");
            return Err(ExitCode::from(exit::GENERAL));
        }
    };
    let client = StatefulMemoryClient::new(channel);
    let stdout_is_tty = std::io::stdout().is_terminal();
    let fmt = Formatter::resolve(output.map(|f| f.to_formatter()), stdout_is_tty);
    Ok((client, detection, fmt))
}
