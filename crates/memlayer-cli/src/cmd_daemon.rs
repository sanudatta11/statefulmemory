//! `daemon` subcommand handlers (FR8, SC-3).
//!
//! - `daemon start [--foreground]`: foreground mode runs the daemon in the
//!   current process (handled directly in `main.rs`); without `--foreground`,
//!   spawn the daemon detached and exit immediately. `daemon start` when
//!   the daemon is already running maps to exit 3 (SC-3).
//! - `daemon stop`: send `Shutdown` RPC. Admin-only in TCP mode (daemon
//!   enforces).
//! - `daemon status`: send `DaemonStatus` RPC. Returns `{stopped: true}` if
//!   the socket is missing — does **not** auto-spawn (that would defeat
//!   the purpose of asking).
//! - `daemon restart`: stop + start.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use memlayer_proto as p;
use serde_json::{json, Value};

use crate::autospawn::{self, AutoSpawnConfig, AutoSpawnError};
use crate::cli::DaemonVerb;
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::{Formatter, Render};

/// Build the auto-spawn config used by both `daemon start` (without
/// `--foreground`) and the implicit auto-spawn in `main::open_client`.
pub fn default_autospawn_config() -> AutoSpawnConfig {
    let socket = memlayer_core::paths::socket_path();
    let lock = memlayer_core::paths::lock_path();
    let log = memlayer_core::paths::log_path();
    let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("memlayer"));
    AutoSpawnConfig::defaults(socket, lock, binary, log)
}

/// Top-level `daemon` dispatch (excluding `start --foreground`, which lives
/// in `main.rs`). Some verbs need a connected client; some don't. We open
/// it lazily here.
pub async fn dispatch(fmt: Formatter, verb: DaemonVerb) -> ExitCode {
    match verb {
        DaemonVerb::Start { foreground } => start(fmt, foreground).await,
        DaemonVerb::Stop => stop(fmt).await,
        DaemonVerb::Status => status(fmt).await,
        DaemonVerb::Restart => restart(fmt).await,
    }
}

async fn start(_fmt: Formatter, foreground: bool) -> ExitCode {
    if foreground {
        // `--foreground` must be handled in main.rs, before we get here.
        unreachable!("daemon start --foreground is dispatched in main.rs");
    }
    let cfg = default_autospawn_config();
    if cfg.socket.exists() {
        // SC-3: already running.
        eprintln!(
            "memlayer: daemon is already running (socket {} exists)",
            cfg.socket.display()
        );
        return ExitCode::from(exit::ALREADY_IN_STATE);
    }
    match autospawn::ensure_running(cfg.clone()).await {
        Ok(()) => {
            eprintln!("daemon started (socket {})", cfg.socket.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("memlayer: {e}");
            ExitCode::from(e.exit_code())
        }
    }
}

async fn stop(_fmt: Formatter) -> ExitCode {
    let socket = memlayer_core::paths::socket_path();
    if !socket.exists() {
        eprintln!("memlayer: daemon is not running (no socket at {})", socket.display());
        return ExitCode::from(exit::ALREADY_IN_STATE);
    }
    let client = match open_client_no_spawn(&socket).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let mut client = client;
    match client.shutdown(p::ShutdownRequest {}).await {
        Ok(_) => {
            // Wait briefly for the socket to disappear so the user knows the
            // shutdown actually happened (FR8 stop semantics).
            wait_for_socket_to_vanish(&socket, Duration::from_secs(5)).await;
            ExitCode::SUCCESS
        }
        Err(s) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

async fn status(fmt: Formatter) -> ExitCode {
    let socket = memlayer_core::paths::socket_path();
    if !socket.exists() {
        // EH path: daemon not running. Return a stable "stopped" payload
        // that scripts can branch on.
        let payload = StoppedStatus { socket: socket.display().to_string() };
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let _ = payload.render(fmt, &mut handle);
        // Exit 4 — daemon not reachable (FR13).
        return ExitCode::from(exit::DAEMON_UNREACHABLE);
    }
    let mut client = match open_client_no_spawn(&socket).await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.daemon_status(p::DaemonStatusRequest {}).await {
        Ok(resp) => {
            let resp = resp.into_inner();
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            let _ = resp.render(fmt, &mut handle);
            ExitCode::SUCCESS
        }
        Err(s) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

async fn restart(fmt: Formatter) -> ExitCode {
    // OQ-4: `daemon restart` with no running daemon → exit 0 (net effect
    // achieved). So missing-socket isn't an error here — proceed straight
    // to spawn.
    let socket = memlayer_core::paths::socket_path();
    if socket.exists() {
        if let Some(c) = open_client_no_spawn(&socket).await.ok() {
            let mut client = c;
            if let Err(s) = client.shutdown(p::ShutdownRequest {}).await {
                eprintln!("memlayer: stop during restart: {}", s.message());
                return ExitCode::from(exit::from_status(s.code()));
            }
            wait_for_socket_to_vanish(&socket, Duration::from_secs(5)).await;
        }
    }
    start(fmt, false).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn open_client_no_spawn(socket: &std::path::Path) -> Result<Client, ExitCode> {
    use memlayer_client::{channel as client_channel, ClientError, MemlayerClient};
    match client_channel::connect_uds(socket) {
        Ok(c) => Ok(MemlayerClient::new(c)),
        Err(ClientError::SocketNotFound(_)) => {
            eprintln!("memlayer: daemon socket disappeared at {}", socket.display());
            Err(ExitCode::from(exit::DAEMON_UNREACHABLE))
        }
        Err(e) => {
            eprintln!("memlayer: {e}");
            Err(ExitCode::from(exit::GENERAL))
        }
    }
}

async fn wait_for_socket_to_vanish(socket: &std::path::Path, budget: Duration) {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if !socket.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `daemon status` payload for the case where the daemon is not running.
#[derive(Debug)]
struct StoppedStatus {
    socket: String,
}

impl Render for StoppedStatus {
    fn render_text(&self, w: &mut dyn io::Write) -> io::Result<()> {
        writeln!(w, "stopped")?;
        writeln!(w, "socket  {}", self.socket)?;
        Ok(())
    }
    fn to_json_value(&self) -> Value {
        json!({ "stopped": true, "socket": self.socket })
    }
}

/// Re-exported to help tests inspect the auto-spawn error → exit-code mapping.
#[allow(dead_code)]
pub fn autospawn_exit_code(err: &AutoSpawnError) -> u8 {
    err.exit_code()
}
