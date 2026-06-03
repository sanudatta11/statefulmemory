//! Domain models — Rust-side projections of SQLite rows.
//!
//! These will be filled out with `From<&rusqlite::Row>` + `Into<proto::*>`
//! conversions in subsequent tasks.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub directory: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub sync_id: String,
    pub session_id: String,
    pub r#type: String,
    pub title: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub scope: String,
    pub created_by: Option<String>,
    pub topic_key: Option<String>,
    pub normalized_hash: Option<String>,
    pub revision_count: i32,
    pub duplicate_count: i32,
    pub last_seen_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub review_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub id: i64,
    pub sync_id: String,
    pub session_id: String,
    pub content: String,
    pub created_at: String,
}

impl Session {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Session {
            id: row.get("id")?,
            directory: row.get("directory")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            summary: row.get("summary")?,
        })
    }
}

impl Observation {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Observation {
            id: row.get("id")?,
            sync_id: row.get("sync_id")?,
            session_id: row.get("session_id")?,
            r#type: row.get("type")?,
            title: row.get("title")?,
            content: row.get("content")?,
            tool_name: row.get("tool_name")?,
            scope: row.get("scope")?,
            created_by: row.get("created_by")?,
            topic_key: row.get("topic_key")?,
            normalized_hash: row.get("normalized_hash")?,
            revision_count: row.get("revision_count")?,
            duplicate_count: row.get("duplicate_count")?,
            last_seen_at: row.get("last_seen_at")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            deleted_at: row.get("deleted_at")?,
            review_after: row.get("review_after")?,
        })
    }
}

impl Prompt {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Prompt {
            id: row.get("id")?,
            sync_id: row.get("sync_id")?,
            session_id: row.get("session_id")?,
            content: row.get("content")?,
            created_at: row.get("created_at")?,
        })
    }
}
