//! Shared types, errors, configuration, and project-name handling for memlayer.
//!
//! This crate is the lowest-level dependency in the workspace. It is depended on
//! by `memlayer-storage`, `memlayer-daemon`, `memlayer-cli`, and `memlayer-sync`.

pub mod config;
pub mod error;
pub mod paths;
pub mod project;
pub mod time;

pub use error::{Error, ErrorKind, Result};

/// Shared by unit tests that mutate process-global `MEMLAYER_*` env vars.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
