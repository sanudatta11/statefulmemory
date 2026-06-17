//! Cross-project global mirror DB.
//!
//! Lives at `~/.memlayer/global.sqlite`. The daemon mirrors every successful
//! `save_observation` from per-project DBs into here so `obs search
//! --all-projects` can serve a single BM25-ranked view without fan-out.
//!
//! Mirror semantics: per-project `(project, source_id)` is unique. Re-saving
//! the same observation is `INSERT OR REPLACE` — last write wins.
//!
//! Failure mode: any global-mirror failure is logged via `tracing` and
//! swallowed; the per-project save (the source of truth) MUST succeed
//! independently of mirror health.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use memlayer_core::error::{Error, Result};

use crate::models::Observation;

mod migrations {
    refinery::embed_migrations!("../../migrations-global");
}

/// Owns the connection to `~/.memlayer/global.sqlite`. One instance per
/// daemon, guarded by a `Mutex` at the call site.
pub struct GlobalDb {
    conn: Connection,
}

/// One row in a `--all-projects` search result. Carries the originating
/// project name so callers can tag the observation.
#[derive(Debug, Clone)]
pub struct GlobalHit {
    pub project: String,
    pub source_id: i64,
    pub r#type: String,
    pub title: String,
    pub content: String,
    pub topic_key: Option<String>,
    pub created_at: String,
    pub bm25: f64,
}

impl GlobalDb {
    /// Open (and migrate) the global DB at `<data_dir>/global.sqlite`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = Self::path_in(data_dir);
        let mut conn = Connection::open(&path)
            .map_err(|e| Error::internal(format!("open_global({}): {e}", path.display())))?;
        // Same pragmas as per-project DBs for consistency.
        crate::pragmas::apply(&conn)?;
        migrations::migrations::runner()
            .run(&mut conn)
            .map_err(|e| Error::internal(format!("global migration failed: {e}")))?;
        Ok(GlobalDb { conn })
    }

    /// Where the global DB file lives within `data_dir`.
    pub fn path_in(data_dir: &Path) -> PathBuf {
        data_dir.join("global.sqlite")
    }

    /// Mirror one observation from a per-project DB into the global DB.
    ///
    /// Idempotent on `(project, source_id)` — re-mirroring is a no-op
    /// in steady state and an UPDATE if any field changed (revision,
    /// content, topic_key).
    pub fn upsert_observation(&mut self, project: &str, obs: &Observation) -> Result<()> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| Error::internal(format!("global upsert: begin tx: {e}")))?;

        tx.execute(
            "INSERT INTO observations
                (project, source_id, type, title, content, topic_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project, source_id) DO UPDATE SET
                type        = excluded.type,
                title       = excluded.title,
                content     = excluded.content,
                topic_key   = excluded.topic_key,
                created_at  = excluded.created_at",
            params![
                project,
                obs.id,
                obs.r#type,
                obs.title,
                obs.content,
                obs.topic_key,
                obs.created_at,
            ],
        )
        .map_err(|e| Error::internal(format!("global upsert: insert observation: {e}")))?;

        // Update manifest with current count + last_synced_at.
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE project = ?1",
                params![project],
                |row| row.get(0),
            )
            .map_err(|e| Error::internal(format!("global upsert: count: {e}")))?;
        tx.execute(
            "INSERT INTO manifest (project, last_synced_at, observation_count)
             VALUES (?1, datetime('now'), ?2)
             ON CONFLICT(project) DO UPDATE SET
                last_synced_at    = excluded.last_synced_at,
                observation_count = excluded.observation_count",
            params![project, count],
        )
        .map_err(|e| Error::internal(format!("global upsert: manifest: {e}")))?;

        tx.commit()
            .map_err(|e| Error::internal(format!("global upsert: commit: {e}")))?;
        Ok(())
    }

    /// BM25-ranked FTS5 search across every project. The `query` string is
    /// passed to FTS5 verbatim — callers should pre-escape if it contains
    /// double-quotes / column qualifiers (mirrors per-project semantics).
    pub fn search(&self, query: &str, limit: i64) -> Result<Vec<GlobalHit>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT o.project, o.source_id, o.type, o.title, o.content,
                        o.topic_key, o.created_at, bm25(observations_fts) AS rank
                 FROM observations o
                 JOIN observations_fts f ON o.global_id = f.rowid
                 WHERE f.observations_fts MATCH ?1
                 ORDER BY rank ASC
                 LIMIT ?2",
            )
            .map_err(|e| Error::internal(format!("global search: prepare: {e}")))?;
        let rows = stmt
            .query_map(params![query, limit], |row| {
                Ok(GlobalHit {
                    project: row.get(0)?,
                    source_id: row.get(1)?,
                    r#type: row.get(2)?,
                    title: row.get(3)?,
                    content: row.get(4)?,
                    topic_key: row.get(5)?,
                    created_at: row.get(6)?,
                    bm25: row.get(7)?,
                })
            })
            .map_err(|e| Error::internal(format!("global search: query_map: {e}")))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::internal(format!("global search: collect: {e}")))?;
        Ok(rows)
    }

    /// Manifest row for a single project. Returns `None` if the project
    /// has not been mirrored yet.
    pub fn manifest_for(&self, project: &str) -> Result<Option<ManifestRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT project, last_synced_at, observation_count
                 FROM manifest WHERE project = ?1",
                params![project],
                |row| {
                    Ok(ManifestRow {
                        project: row.get(0)?,
                        last_synced_at: row.get(1)?,
                        observation_count: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|e| Error::internal(format!("global manifest_for: {e}")))?;
        Ok(row)
    }
}

/// One row of the global `manifest` table.
#[derive(Debug, Clone)]
pub struct ManifestRow {
    pub project: String,
    pub last_synced_at: String,
    pub observation_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn obs(id: i64, title: &str, content: &str) -> Observation {
        Observation {
            id,
            sync_id: format!("sync-{id}"),
            session_id: "sess-1".into(),
            r#type: "decision".into(),
            title: title.into(),
            content: content.into(),
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 1,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2026-06-17T00:00:00Z".into(),
            updated_at: "2026-06-17T00:00:00Z".into(),
            deleted_at: None,
            review_after: None,
            superseded_ids: vec![],
        }
    }

    #[test]
    fn open_runs_v1_migration() {
        let dir = TempDir::new().unwrap();
        let _db = GlobalDb::open(dir.path()).expect("migrate");
        let conn = Connection::open(GlobalDb::path_in(dir.path())).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name IN ('observations','manifest')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 2);
    }

    #[test]
    fn upsert_replaces_on_duplicate_project_source_id() {
        let dir = TempDir::new().unwrap();
        let mut db = GlobalDb::open(dir.path()).unwrap();
        db.upsert_observation("repo-a", &obs(1, "first", "v1")).unwrap();
        db.upsert_observation("repo-a", &obs(1, "first", "v2")).unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE project='repo-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "duplicate (project, source_id) must REPLACE");

        let content: String = db
            .conn
            .query_row(
                "SELECT content FROM observations WHERE project='repo-a' AND source_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content, "v2");
    }

    #[test]
    fn search_returns_bm25_ranked_hits_across_projects() {
        let dir = TempDir::new().unwrap();
        let mut db = GlobalDb::open(dir.path()).unwrap();
        db.upsert_observation("repo-a", &obs(1, "use pgx not gorm", "team prefers raw sql"))
            .unwrap();
        db.upsert_observation("repo-b", &obs(2, "kafka topic naming", "kebab case"))
            .unwrap();
        db.upsert_observation("repo-a", &obs(3, "auth via jwt", "validate in middleware"))
            .unwrap();

        let hits = db.search("pgx", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].project, "repo-a");
        assert_eq!(hits[0].source_id, 1);

        let hits = db.search("kafka OR jwt", 5).unwrap();
        assert_eq!(hits.len(), 2);
        let projects: Vec<_> = hits.iter().map(|h| h.project.as_str()).collect();
        assert!(projects.contains(&"repo-a"));
        assert!(projects.contains(&"repo-b"));
    }

    #[test]
    fn manifest_tracks_per_project_count() {
        let dir = TempDir::new().unwrap();
        let mut db = GlobalDb::open(dir.path()).unwrap();
        db.upsert_observation("repo-a", &obs(1, "t1", "c1")).unwrap();
        db.upsert_observation("repo-a", &obs(2, "t2", "c2")).unwrap();
        db.upsert_observation("repo-b", &obs(1, "x", "y")).unwrap();

        let a = db.manifest_for("repo-a").unwrap().unwrap();
        let b = db.manifest_for("repo-b").unwrap().unwrap();
        assert_eq!(a.observation_count, 2);
        assert_eq!(b.observation_count, 1);
    }
}
