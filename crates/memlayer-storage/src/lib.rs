// Generated with AI Coding Rules Hub
//! Per-project SQLite+FTS5 storage for memlayer.
//!
//! Each project gets its own database file at `~/.memlayer/projects/<id>.db`.
//! A single dedicated `std::thread` owns the write connection for that project;
//! reads come from a small per-project `Connection` pool.
//!
//! Migrations live in `../../migrations/` and are embedded at compile time via
//! `refinery::embed_migrations!`. Every connection sets the pragmas in
//! `pragmas::apply_pragmas` (PRD §5.2).

pub mod cursor;
pub mod db;
pub mod dedupe;
pub mod diskmon;
pub mod facts;
pub mod global;
pub mod models;
pub mod pragmas;
pub mod projects_admin;
pub mod prompts;
pub mod read;
pub mod registry;
pub mod sessions;
pub mod stats;
pub mod sync_state;
pub mod write;

pub use db::{open_read, open_write, Migrate};
pub use global::{GlobalDb, GlobalHit, ManifestRow};
pub use models::{Observation, Prompt, Session};
pub use registry::{ProjectConfig, ProjectRegistry, ProjectState};
pub use sync_state::{ExportedIds, UpsertOutcome};

mod migrations {
    refinery::embed_migrations!("../../migrations");
}

/// Run all pending migrations against an open connection.
pub fn run_migrations(conn: &mut rusqlite::Connection) -> memlayer_core::Result<()> {
    migrations::migrations::runner()
        .run(conn)
        .map_err(|e| memlayer_core::Error::internal(format!("migration failed: {e}")))?;
    Ok(())
}
