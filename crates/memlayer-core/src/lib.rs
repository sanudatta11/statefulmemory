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
