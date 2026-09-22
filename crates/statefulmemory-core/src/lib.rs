//! Shared types, errors, configuration, and project-name handling for statefulmemory.
//!
//! This crate is the lowest-level dependency in the workspace. It is depended on
//! by `statefulmemory-storage`, `statefulmemory-daemon`, `statefulmemory-cli`, and `statefulmemory-sync`.

pub mod config;
pub mod error;
pub mod git;
pub mod paths;
pub mod process;
pub mod project;
pub mod time;
pub mod tokens;

pub use config::{normalize_entity_name, EdgeRelation, Entity, EntityKind, MentionSource};
pub use error::{Error, ErrorKind, Result};

/// Shared by unit tests that mutate process-global `STATEFULMEMORY_*` env vars.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
