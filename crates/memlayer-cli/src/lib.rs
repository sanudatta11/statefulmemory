//! memlayer CLI: clap dispatch, output formatting, project detection,
//! daemon auto-spawn, and exit-code mapping.
//!
//! The binary entry point in `src/main.rs` builds a tokio runtime, parses the
//! [`cli::Cli`] root, and dispatches to per-noun command handlers. Cross-
//! cutting concerns — output formatting (FR1.2), `--no-color` (FR1.4),
//! `MEMLAYER_PROJECT` env (FR3.4), auto-spawn (FR2), exit codes (FR13) — live
//! here so each command module added in spec2-t5..t7 stays focused on a
//! single RPC.

pub mod autospawn;
pub mod cli;
pub mod cmd_obs;
pub mod cmd_project;
pub mod cmd_prompt;
pub mod cmd_session;
pub mod cmd_sync;
pub mod exit;
pub mod formatter;
pub mod project_detect;
pub mod render;
