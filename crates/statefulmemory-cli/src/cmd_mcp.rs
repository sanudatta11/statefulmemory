//! `statefulmemory mcp` — launch the local stdio MCP server.
//!
//! Resolves the cwd project and daemon socket (same logic as the RPC
//! subcommands), makes a best-effort attempt to auto-spawn the daemon, then
//! hands control to `statefulmemory_mcp::serve`, which runs until stdin closes.
//!
//! Unlike `open_client`, a failed auto-spawn does NOT abort: the MCP server
//! still starts so its `memory_health` tool can report the problem.

use std::process::ExitCode;
use std::sync::Arc;

use crate::{autospawn, cmd_daemon, exit, project_detect};

pub async fn dispatch(project_flag: Option<String>, args: crate::cli::McpArgs) -> ExitCode {
    let cli_override = project_flag.filter(|s| !s.is_empty());
    let env_override = std::env::var("STATEFULMEMORY_PROJECT")
        .ok()
        .filter(|s| !s.is_empty());

    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("statefulmemory mcp: getcwd: {e}");
            return ExitCode::from(exit::GENERAL);
        }
    };

    let project = match project_detect::detect_in(&cwd, env_override, cli_override) {
        Ok(d) => d.normalized,
        Err(project_detect::DetectError::Ambiguous) => {
            eprintln!(
                "statefulmemory mcp: project is ambiguous; using default for health diagnostics; pass --project or STATEFULMEMORY_PROJECT for scoped tools"
            );
            "default".to_string()
        }
        Err(e) => {
            eprintln!("statefulmemory mcp: {e}");
            return ExitCode::from(e.exit_code());
        }
    };

    let socket = statefulmemory_core::paths::socket_path();

    // Explicit --remote flags take precedence over env; otherwise fall back to
    // STATEFULMEMORY_ADDR-or-local-UDS resolution (Phase 4.1/4.2).
    let endpoint = match endpoint_from_flags(&args, &socket) {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("statefulmemory mcp: {e}; falling back to local UDS");
            statefulmemory_client::Endpoint::Uds(socket.clone())
        }
    };

    let cfg = cmd_daemon::default_autospawn_config();

    // Local only: ensure the daemon (best-effort — if it fails we still start
    // so memory_health can diagnose) and enable mid-session respawn recovery.
    // Remote endpoints can't be locally spawned/respawned, so skip both.
    let recover: Option<Arc<dyn statefulmemory_mcp::client::Recovery>> = if endpoint.is_remote() {
        eprintln!(
            "statefulmemory mcp: targeting remote daemon via STATEFULMEMORY_ADDR; \
             skipping local auto-spawn (use memory_health to verify connectivity)"
        );
        None
    } else {
        match autospawn::ensure_running(&cfg).await {
            Ok(_) if !autospawn::probe(&socket).await => {
                eprintln!(
                    "statefulmemory mcp: daemon socket appeared but health probe failed; starting anyway — use memory_health to diagnose"
                );
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "statefulmemory mcp: daemon auto-spawn failed ({e}); starting anyway — \
                     use the memory_health tool to diagnose"
                );
            }
        }
        Some(Arc::new(statefulmemory_mcp::EnsureRecovery::new(cfg)))
    };

    // Fallback client identity used when MCP initialize has not supplied
    // clientInfo yet. memory_add prefers the negotiated Peer clientInfo
    // (name@version) as created_by when available.
    let client_info = format!("statefulmemory-mcp/{}", env!("CARGO_PKG_VERSION"));

    match statefulmemory_mcp::serve(endpoint, project, client_info, recover).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("statefulmemory mcp: {e}");
            ExitCode::from(exit::GENERAL)
        }
    }
}

/// Build the daemon endpoint from `statefulmemory mcp` flags, falling back to the
/// env/UDS resolution when `--remote` is absent. `--remote` requires `--ca`
/// and `--token-env` (the token is read from that env var, never from argv).
fn endpoint_from_flags(
    args: &crate::cli::McpArgs,
    socket: &std::path::Path,
) -> Result<statefulmemory_client::Endpoint, String> {
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
            Ok(statefulmemory_client::Endpoint::Tcp {
                addr: addr.trim().to_string(),
                ca_pem,
                token,
                domain,
            })
        }
        None => statefulmemory_client::Endpoint::from_env_or_uds(socket.to_path_buf())
            .map_err(|e| e.to_string()),
    }
}
