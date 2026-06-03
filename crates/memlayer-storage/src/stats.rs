//! `Stats` RPC — per-project row counts.
//!
//! Spec section: FR5.4.

use rusqlite::Connection;

use memlayer_core::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ProjectStats {
    pub project: String,
    pub observations: i64,
    pub soft_deleted_observations: i64,
    pub sessions: i64,
    pub prompts: i64,
}

pub fn count_for(conn: &Connection, project: &str) -> Result<ProjectStats> {
    let observations: i64 = conn
        .query_row(
            "SELECT count(*) FROM observations WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| Error::internal(format!("count active obs: {e}")))?;
    let soft_deleted: i64 = conn
        .query_row(
            "SELECT count(*) FROM observations WHERE deleted_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| Error::internal(format!("count deleted obs: {e}")))?;
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .map_err(|e| Error::internal(format!("count sessions: {e}")))?;
    let prompts: i64 = conn
        .query_row("SELECT count(*) FROM user_prompts", [], |r| r.get(0))
        .map_err(|e| Error::internal(format!("count prompts: {e}")))?;
    Ok(ProjectStats {
        project: project.into(),
        observations,
        soft_deleted_observations: soft_deleted,
        sessions,
        prompts,
    })
}
