//! `memlayer mcp` — launch the local stdio MCP server.
//!
//! Resolves the cwd project and daemon socket (same logic as the RPC
//! subcommands), makes a best-effort attempt to auto-spawn the daemon, then
//! hands control to `memlayer_mcp::serve`, which runs until stdin closes.
//!
//! Unlike `open_client`, a failed auto-spawn does NOT abort: the MCP server
//! still starts so its `memory_health` tool can report the problem.

use std::process::ExitCode;

use crate::{autospawn, cmd_daemon, exit, project_detect};

pub async fn dispatch(project_flag: Option<String>) -> ExitCode {
    let cli_override = project_flag.filter(|s| !s.is_empty());
    let env_override = std::env::var("MEMLAYER_PROJECT")
        .ok()
        .filter(|s| !s.is_empty());

    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("memlayer mcp: getcwd: {e}");
            return ExitCode::from(exit::GENERAL);
        }
    };

    let detection = match project_detect::detect_in(&cwd, env_override, cli_override) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("memlayer mcp: {e}");
            return ExitCode::from(e.exit_code());
        }
    };

    let socket = memlayer_core::paths::socket_path();

    // Best-effort auto-spawn. If it fails we still start the server so the
    // agent can call memory_health to diagnose (do not abort).
    if !socket.exists() {
        if let Err(e) = autospawn::ensure_running(cmd_daemon::default_autospawn_config()).await {
            eprintln!(
                "memlayer mcp: daemon auto-spawn failed ({e}); starting anyway — \
                 use the memory_health tool to diagnose"
            );
        }
    }

    // Fallback client identity; the negotiated MCP clientInfo (the actual
    // calling agent) is captured per-call from the request context.
    let client_info = format!("memlayer-mcp/{}", env!("CARGO_PKG_VERSION"));

    match memlayer_mcp::serve(socket, detection.normalized, client_info).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("memlayer mcp: {e}");
            ExitCode::from(exit::GENERAL)
        }
    }
}
