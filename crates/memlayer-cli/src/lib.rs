//! memlayer CLI: clap dispatch, output formatting, project detection,
//! daemon auto-spawn, and exit-code mapping.
//!
//! The binary entry point in `src/main.rs` builds a tokio runtime, parses the
//! [`cli::Cli`] root, and dispatches to per-noun command handlers. Cross-
//! cutting concerns — output formatting (FR1.2), `--no-color` (FR1.4),
//! `MEMLAYER_PROJECT` env (FR3.4), `--quiet` (FR1.6), auto-spawn (FR2),
//! exit codes (FR13) — live here so each command module added in spec2-t5..t7
//! stays focused on a single RPC.

use std::sync::atomic::{AtomicBool, Ordering};

/// Shared by unit tests that mutate process-global `MEMLAYER_*` / `NO_COLOR` env.
/// Each module used to have its own mutex, which still raced across modules.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod agents;
pub mod audit;
pub mod autospawn;
pub mod cli;
pub mod cmd_daemon;
pub mod cmd_doctor;
pub mod cmd_eval;
pub mod cmd_hook;
pub mod cmd_logs;
pub mod cmd_mcp;
pub mod cmd_obs;
pub mod cmd_config;
pub mod cmd_project;
pub mod cmd_prompt;
pub mod cmd_session;
pub mod cmd_skill;
pub mod cmd_tui;
pub mod cmd_uninstall;
pub mod cmd_sync;
pub mod cmd_team;
pub mod cmd_version;
pub mod exit;
pub mod formatter;
pub mod mcp_install;
pub mod project_detect;
pub mod render;

/// Process-global `--quiet` flag (FR1.6). Set once at startup by `main`,
/// read by [`info`] before printing informational output. Errors and the
/// final RPC payload always print regardless of this flag.
static QUIET: AtomicBool = AtomicBool::new(false);

/// Initialize the global `--quiet` and `--no-color` flags. Called once
/// from `main` after CLI parsing. `--no-color` is implemented by setting
/// the `NO_COLOR` environment variable so [`formatter::use_color`] sees it
/// — this composes correctly with users who set `NO_COLOR` directly.
pub fn init_globals(quiet: bool, no_color: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
    if no_color && std::env::var_os("NO_COLOR").is_none() {
        // SAFETY: set_var is called once at startup before any threads spawn
        // and before any handler runs. All downstream reads of NO_COLOR go
        // through std::env::var_os which is sync.
        std::env::set_var("NO_COLOR", "1");
    }
}

/// Whether informational stderr output should be suppressed. Errors are
/// never suppressed.
pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// Print an informational message to stderr if `--quiet` is not set.
/// Errors must still use `eprintln!` directly — [`info`] is for the
/// "daemon started", "wrote ca.pem", etc. lines that the user can ask
/// to be silenced.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {{
        if !$crate::quiet() {
            eprintln!($($arg)*);
        }
    }};
}

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod globals_tests {
    use super::*;

    /// FR1.4: `--no-color` must propagate to anything that consults the
    /// `NO_COLOR` environment variable. `init_globals(_, no_color=true)`
    /// is required to set the env var when not already set.
    #[test]
    fn init_globals_no_color_sets_env_var() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Snapshot and clear the env so the test is hermetic regardless of
        // how the developer's shell is configured.
        let original = std::env::var_os("NO_COLOR");
        std::env::remove_var("NO_COLOR");

        init_globals(false, true);
        assert!(
            std::env::var_os("NO_COLOR").is_some(),
            "--no-color must export NO_COLOR for downstream consumers",
        );

        // Restore for any subsequent test in the same process.
        match original {
            Some(v) => std::env::set_var("NO_COLOR", v),
            None => std::env::remove_var("NO_COLOR"),
        }
    }

    #[test]
    fn init_globals_quiet_flips_quiet_flag() {
        // Reset to a known state.
        QUIET.store(false, Ordering::Relaxed);
        assert!(!quiet());

        init_globals(true, false);
        assert!(quiet(), "--quiet must flip the global QUIET flag");

        init_globals(false, false);
        assert!(!quiet(), "init_globals(false, _) must clear quiet");
    }
}
