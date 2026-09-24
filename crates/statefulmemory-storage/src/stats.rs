//! `Stats` RPC — per-project row counts.
//!
//! Spec section: FR5.4.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use statefulmemory_core::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectStats {
    pub project: String,
    pub observations: i64,
    pub soft_deleted_observations: i64,
    pub sessions: i64,
    pub prompts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSummary {
    pub project: String,
    pub observations: i64,
    pub soft_deleted_observations: i64,
    pub sessions: i64,
    pub prompts: i64,
    pub facts: i64,
    pub active_facts: i64,
    pub embedded_observations: i64,
    pub anchor_rows: i64,
    pub anchored_observations: i64,
    pub entities: i64,
    pub entity_mentions: i64,
    pub entity_edges: i64,
    pub jobs: i64,
    pub pending_jobs: i64,
    pub running_jobs: i64,
    pub dead_jobs: i64,
    pub latest_observation_at: Option<String>,
    pub latest_updated_at: Option<String>,
}

impl ProjectSummary {
    pub fn active_observations(&self) -> i64 {
        self.observations
    }

    pub fn total_observations(&self) -> i64 {
        self.observations.saturating_add(self.soft_deleted_observations)
    }
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

pub fn project_summary(conn: &Connection, project: &str) -> Result<ProjectSummary> {
    let sql = "
        SELECT
            (SELECT COUNT(*) FROM observations WHERE deleted_at IS NULL),
            (SELECT COUNT(*) FROM observations WHERE deleted_at IS NOT NULL),
            (SELECT COUNT(*) FROM sessions),
            (SELECT COUNT(*) FROM user_prompts),
            (SELECT COUNT(*) FROM facts),
            (SELECT COUNT(*)
             FROM facts f
             JOIN observations o ON o.id = f.obs_id
             WHERE f.superseded_by IS NULL AND o.deleted_at IS NULL),
            (SELECT COUNT(DISTINCT m.observation_id)
             FROM observation_embedding_meta m
             JOIN observations o ON o.id = m.observation_id
             WHERE o.deleted_at IS NULL),
            (SELECT COUNT(*) FROM observation_anchors),
            (SELECT COUNT(DISTINCT a.observation_id)
             FROM observation_anchors a
             JOIN observations o ON o.id = a.observation_id
             WHERE o.deleted_at IS NULL),
            (SELECT COUNT(*) FROM entities),
            (SELECT COUNT(*) FROM entity_mentions),
            (SELECT COUNT(*) FROM entity_edges),
            (SELECT COUNT(*) FROM jobs),
            (SELECT COUNT(*) FROM jobs WHERE status = 'pending'),
            (SELECT COUNT(*) FROM jobs WHERE status = 'running'),
            (SELECT COUNT(*) FROM jobs WHERE status = 'dead'),
            (SELECT MAX(created_at) FROM observations WHERE deleted_at IS NULL),
            (SELECT MAX(updated_at) FROM observations WHERE deleted_at IS NULL)
    ";
    conn.query_row(sql, [], |row| {
        Ok(ProjectSummary {
            project: project.to_string(),
            observations: row.get(0)?,
            soft_deleted_observations: row.get(1)?,
            sessions: row.get(2)?,
            prompts: row.get(3)?,
            facts: row.get(4)?,
            active_facts: row.get(5)?,
            embedded_observations: row.get(6)?,
            anchor_rows: row.get(7)?,
            anchored_observations: row.get(8)?,
            entities: row.get(9)?,
            entity_mentions: row.get(10)?,
            entity_edges: row.get(11)?,
            jobs: row.get(12)?,
            pending_jobs: row.get(13)?,
            running_jobs: row.get(14)?,
            dead_jobs: row.get(15)?,
            latest_observation_at: row.get(16)?,
            latest_updated_at: row.get(17)?,
        })
    })
    .map_err(|e| Error::internal(format!("project summary: {e}")))
}

pub fn summary(conn: &Connection, project: &str) -> Result<ProjectSummary> {
    project_summary(conn, project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_write;
    use rusqlite::params;
    use tempfile::TempDir;

    fn seed_observation(conn: &Connection, id: i64, deleted: bool) {
        conn.execute(
            "INSERT INTO observations
                (id, sync_id, session_id, type, title, content, scope, deleted_at)
             VALUES (?1, ?2, 's1', 'note', ?3, ?4, 'project', ?5)",
            params![
                id,
                format!("sync-{id}"),
                format!("title-{id}"),
                format!("content-{id}"),
                deleted.then_some("2026-01-01 00:00:00"),
            ],
        )
        .unwrap();
    }

    #[test]
    fn project_summary_aggregates_source_and_projection_rows() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("summary.db")).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        seed_observation(&conn, 1, false);
        seed_observation(&conn, 2, true);
        conn.execute(
            "INSERT INTO facts (obs_id, subject, predicate, object, extracted_by)
             VALUES (1, 'subject', 'uses', 'object', 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observation_embedding_meta (observation_id, model, dim)
             VALUES (1, 'test', 384)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observation_anchors (observation_id, path) VALUES (1, 'src/lib.rs')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entities (kind, name, norm_name)
             VALUES ('file', 'src/lib.rs', 'src/lib.rs')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (kind, project_name, payload) VALUES ('embed', 'p', '{}')",
            [],
        )
        .unwrap();

        let summary = project_summary(&conn, "p").unwrap();
        assert_eq!(summary.project, "p");
        assert_eq!(summary.observations, 1);
        assert_eq!(summary.soft_deleted_observations, 1);
        assert_eq!(summary.sessions, 1);
        assert_eq!(summary.prompts, 0);
        assert_eq!(summary.facts, 1);
        assert_eq!(summary.active_facts, 1);
        assert_eq!(summary.embedded_observations, 1);
        assert_eq!(summary.anchor_rows, 1);
        assert_eq!(summary.anchored_observations, 1);
        assert_eq!(summary.entities, 1);
        assert_eq!(summary.entity_mentions, 0);
        assert_eq!(summary.entity_edges, 0);
        assert_eq!(summary.jobs, 1);
        assert_eq!(summary.pending_jobs, 1);
        assert_eq!(summary.running_jobs, 0);
        assert_eq!(summary.dead_jobs, 0);
        assert!(summary.latest_observation_at.is_some());
        assert!(summary.latest_updated_at.is_some());
    }

    #[test]
    fn project_summary_handles_empty_database() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("summary.db")).unwrap();
        let summary = project_summary(&conn, "empty").unwrap();
        assert_eq!(summary.observations, 0);
        assert_eq!(summary.facts, 0);
        assert_eq!(summary.jobs, 0);
        assert_eq!(summary.latest_observation_at, None);
    }
}
