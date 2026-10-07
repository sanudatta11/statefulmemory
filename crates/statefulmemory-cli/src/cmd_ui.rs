use std::process::ExitCode;
use std::sync::Arc;

use statefulmemory_client::Endpoint;
use statefulmemory_ui::{GrpcBackend, UiAssets, UiBackend, UiConfig};

use crate::cli::UiArgs;
use crate::cmd_daemon::default_autospawn_config;
use crate::exit;

const UI_HTML: &str = include_str!("../../../dashboard/dist/index.html");
const UI_SCRIPT: &str = include_str!("../../../dashboard/dist/assets/app.js");
const UI_STYLE: &str = include_str!("../../../dashboard/dist/assets/app.css");

pub async fn dispatch(project_flag: Option<String>, args: &UiArgs) -> ExitCode {
    let config = match UiConfig::new(args.host.clone(), args.port) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::USAGE);
        }
    };
    let project = {
        let env_override = std::env::var("STATEFULMEMORY_PROJECT")
            .ok()
            .filter(|value| !value.is_empty());
        let cwd = match std::env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("statefulmemory: getcwd: {error}");
                return ExitCode::from(exit::GENERAL);
            }
        };
        match crate::project_detect::detect_in(&cwd, env_override, project_flag) {
            Ok(detection) => detection.normalized,
            Err(error) => {
                eprintln!("statefulmemory: {error}");
                return ExitCode::from(error.exit_code());
            }
        }
    };

    let socket = statefulmemory_core::paths::socket_path();

    // --remote flags take precedence over env; otherwise STATEFULMEMORY_ADDR-or-UDS
    // resolution (Phase 4.1 Endpoint abstraction, mirroring `statefulmemory mcp`).
    let endpoint = match endpoint_from_flags(args, &socket) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::USAGE);
        }
    };

    // Local only: ensure the daemon is up. A remote team daemon can't be
    // auto-spawned, so skip it and let connect() surface a dial error.
    if !endpoint.is_remote() {
        if let Err(error) = crate::autospawn::ensure_running(&default_autospawn_config()).await {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(error.exit_code());
        }
    }

    let backend: Arc<dyn UiBackend> = match GrpcBackend::connect(&endpoint).await {
        Ok(backend) => Arc::new(backend),
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::DAEMON_UNREACHABLE);
        }
    };
    let remote = endpoint.is_remote();
    let running = match statefulmemory_ui::bind_with_assets(
        config,
        project,
        backend,
        UiAssets {
            html: UI_HTML,
            script: UI_SCRIPT,
            style: UI_STYLE,
        },
        remote,
    ) {
        Ok(running) => running,
        Err(error) => {
            eprintln!("statefulmemory: {error}");
            return ExitCode::from(exit::GENERAL);
        }
    };
    let source = if remote {
        "remote team daemon"
    } else {
        "local daemon"
    };
    println!(
        "statefulmemory UI ({source}) → {}     hit Ctrl-C to stop",
        running.launch_url()
    );
    let thread = std::thread::spawn(move || running.serve());
    match thread.join() {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("statefulmemory: ui: {error}");
            ExitCode::from(exit::GENERAL)
        }
        Err(_) => {
            eprintln!("statefulmemory: ui worker terminated unexpectedly");
            ExitCode::from(exit::GENERAL)
        }
    }
}

/// Build the daemon endpoint from `statefulmemory ui` flags, falling back to the
/// env/UDS resolution when `--remote` is absent. `--remote` requires `--ca` and
/// `--token-env` (the token is read from that env var, never from argv).
fn endpoint_from_flags(args: &UiArgs, socket: &std::path::Path) -> Result<Endpoint, String> {
    match args.remote.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(addr) => {
            let ca_path = args.ca.as_ref().ok_or("--remote requires --ca <ca.pem>")?;
            let ca_pem = std::fs::read(ca_path)
                .map_err(|e| format!("read --ca {}: {e}", ca_path.display()))?;
            let token_env = args
                .token_env
                .as_deref()
                .ok_or("--remote requires --token-env <ENV_VAR>")?;
            let token = std::env::var(token_env)
                .map_err(|_| format!("--token-env {token_env} is not set in the environment"))?;
            let domain = std::env::var("STATEFULMEMORY_TLS_DOMAIN")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "localhost".to_string());
            Ok(Endpoint::Tcp {
                addr: addr.trim().to_string(),
                ca_pem,
                token,
                domain,
            })
        }
        None => Endpoint::from_env_or_uds(socket.to_path_buf()).map_err(|e| e.to_string()),
    }
}
