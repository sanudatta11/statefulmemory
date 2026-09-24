//! Doctor auto-repair and health audit module for statefulmemory storage.

use statefulmemory_core::error::Error;
use statefulmemory_core::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorFinding {
    pub code: String,
    pub severity: String, // "info" | "warn" | "error"
    pub message: String,
    pub remedy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectionHealthSnapshot {
    pub database_integrity: String,
    pub total_observations: i64,
    pub active_observations: i64,
    pub soft_deleted_observations: i64,
    pub fts_rows: i64,
    pub fts_missing: i64,
    pub fts_stale: i64,
    pub embedding_rows: i64,
    pub embedding_missing: i64,
    pub orphan_embeddings: i64,
    pub fact_rows: i64,
    pub fact_observations: i64,
    pub anchor_rows: i64,
    pub anchored_observations: i64,
    pub graph: crate::graph::GraphCoverage,
    pub jobs: crate::jobs::JobStatusCounts,
    pub verify_states: Vec<(String, i64)>,
}

impl ProjectionHealthSnapshot {
    pub fn drift(&self) -> i64 {
        self.fts_missing
            .saturating_add(self.fts_stale)
            .saturating_add(self.embedding_missing)
            .saturating_add(self.orphan_embeddings)
    }

    pub fn is_healthy(&self) -> bool {
        self.database_integrity == "ok" && self.drift() == 0 && self.jobs.dead == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthSnapshot {
    pub stats: crate::stats::ProjectStats,
    pub projections: ProjectionHealthSnapshot,
    pub jobs: crate::jobs::JobStatusCounts,
}

impl HealthSnapshot {
    pub fn is_healthy(&self) -> bool {
        self.projections.is_healthy()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectHealthSnapshot {
    pub project: String,
    pub stats: crate::stats::ProjectStats,
    pub projections: ProjectionHealthSnapshot,
}

pub fn projection_health_snapshot(conn: &Connection) -> Result<ProjectionHealthSnapshot> {
    let database_integrity: String = conn
        .query_row("PRAGMA integrity_check(10)", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("database integrity check: {e}")))?;
    let total_observations: i64 = conn
        .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("count observations: {e}")))?;
    let active_observations: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count active observations: {e}")))?;
    let soft_deleted_observations = total_observations.saturating_sub(active_observations);
    let fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM observations_fts", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("count fts rows: {e}")))?;
    let fts_missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observations o
             WHERE o.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM observations_fts f WHERE f.rowid = o.id)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count missing fts rows: {e}")))?;
    let fts_stale: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observations_fts f
             WHERE NOT EXISTS (SELECT 1 FROM observations o WHERE o.id = f.rowid)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count stale fts rows: {e}")))?;
    let embedding_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observation_embedding_meta",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count embedding rows: {e}")))?;
    let embedding_missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observations o
             WHERE o.deleted_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM observation_embedding_meta m
                   WHERE m.observation_id = o.id
               )",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count missing embeddings: {e}")))?;
    let orphan_embeddings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observation_embedding_meta m
             WHERE NOT EXISTS (SELECT 1 FROM observations o WHERE o.id = m.observation_id)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count orphan embeddings: {e}")))?;
    let fact_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("count facts: {e}")))?;
    let fact_observations: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT f.obs_id) FROM facts f
             JOIN observations o ON o.id = f.obs_id
             WHERE o.deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count fact observations: {e}")))?;
    let anchor_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM observation_anchors", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("count anchors: {e}")))?;
    let anchored_observations: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT a.observation_id) FROM observation_anchors a
             JOIN observations o ON o.id = a.observation_id
             WHERE o.deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("count anchored observations: {e}")))?;
    let mut verify_stmt = conn
        .prepare(
            "SELECT verify_state, COUNT(*) FROM observations
             GROUP BY verify_state ORDER BY verify_state",
        )
        .map_err(|e| Error::internal(format!("prepare verify state counts: {e}")))?;
    let verify_rows = verify_stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .map_err(|e| Error::internal(format!("query verify state counts: {e}")))?;
    let mut verify_states = Vec::new();
    for row in verify_rows {
        verify_states.push(row.map_err(|e| Error::internal(format!("read verify state counts: {e}")))?);
    }
    Ok(ProjectionHealthSnapshot {
        database_integrity,
        total_observations,
        active_observations,
        soft_deleted_observations,
        fts_rows,
        fts_missing,
        fts_stale,
        embedding_rows,
        embedding_missing,
        orphan_embeddings,
        fact_rows,
        fact_observations,
        anchor_rows,
        anchored_observations,
        graph: crate::graph::coverage(conn)?,
        jobs: crate::jobs::status_summary(conn)?,
        verify_states,
    })
}

pub fn projection_health(conn: &Connection) -> Result<ProjectionHealthSnapshot> {
    projection_health_snapshot(conn)
}

pub fn project_health_snapshot(
    conn: &Connection,
    project_name: &str,
) -> Result<ProjectHealthSnapshot> {
    let stats = crate::stats::count_for(conn, project_name)?;
    Ok(ProjectHealthSnapshot {
        project: project_name.to_string(),
        stats: stats.clone(),
        projections: projection_health_snapshot(conn)?,
    })
}

pub fn health_snapshot(conn: &Connection) -> Result<HealthSnapshot> {
    let projections = projection_health_snapshot(conn)?;
    Ok(HealthSnapshot {
        stats: crate::stats::count_for(conn, "local")?,
        jobs: projections.jobs.clone(),
        projections,
    })
}

/// Audit database health and optionally perform automatic non-destructive repairs.
pub fn audit_and_repair(conn: &Connection, auto_repair: bool) -> Result<Vec<DoctorFinding>> {
    let mut findings = Vec::new();

    // 1. Check schema meta version
    let schema_ver: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some(v) = schema_ver {
        findings.push(DoctorFinding {
            code: "SCHEMA_VERSION".into(),
            severity: "info".into(),
            message: format!("SQLite schema version is {v}"),
            remedy: None,
        });
    } else {
        findings.push(DoctorFinding {
            code: "SCHEMA_VERSION".into(),
            severity: "warn".into(),
            message: "Missing schema_meta version key".into(),
            remedy: None,
        });
    }

    // 2. Check SQLite Database Integrity
    let integrity: std::result::Result<String, _> =
        conn.query_row("PRAGMA integrity_check(10)", [], |r| r.get(0));
    match integrity {
        Ok(res) if res == "ok" => {
            findings.push(DoctorFinding {
                code: "DB_INTEGRITY".into(),
                severity: "info".into(),
                message: "Database integrity check passed".into(),
                remedy: None,
            });
        }
        Ok(res) => {
            findings.push(DoctorFinding {
                code: "DB_INTEGRITY".into(),
                severity: "error".into(),
                message: format!("Database integrity issue: {res}"),
                remedy: Some("Restore database from backup or re-export project".into()),
            });
        }
        Err(e) => {
            findings.push(DoctorFinding {
                code: "DB_INTEGRITY".into(),
                severity: "error".into(),
                message: format!("Failed to run SQLite integrity check: {e}"),
                remedy: None,
            });
        }
    }

    // 3. Check FTS5 index consistency
    let obs_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let fts_count: std::result::Result<i64, _> =
        conn.query_row("SELECT COUNT(*) FROM observations_fts", [], |r| r.get(0));

    match fts_count {
        Ok(fc) => {
            if fc < obs_count {
                let msg = format!(
                    "FTS index row count ({fc}) is less than active observations count ({obs_count})"
                );
                if auto_repair {
                    let repair_res = conn.execute(
                        "INSERT INTO observations_fts(observations_fts) VALUES('rebuild')",
                        [],
                    );
                    let remedy_text = match repair_res {
                        Ok(_) => "Repaired: Rebuilt FTS5 index successfully".to_string(),
                        Err(e) => format!("Repair failed: {e}"),
                    };
                    findings.push(DoctorFinding {
                        code: "FTS5_DESYNC".into(),
                        severity: "warn".into(),
                        message: msg,
                        remedy: Some(remedy_text),
                    });
                } else {
                    findings.push(DoctorFinding {
                        code: "FTS5_DESYNC".into(),
                        severity: "warn".into(),
                        message: msg,
                        remedy: Some(
                            "Run `statefulmemory doctor --repair` to rebuild the FTS5 index".into(),
                        ),
                    });
                }
            } else {
                findings.push(DoctorFinding {
                    code: "FTS5_DESYNC".into(),
                    severity: "info".into(),
                    message: format!("FTS index synchronized ({fc} entries)"),
                    remedy: None,
                });
            }
        }
        Err(e) => {
            let msg = format!("Failed to query FTS5 index: {e}");
            if auto_repair {
                let repair_res = conn.execute(
                    "INSERT INTO observations_fts(observations_fts) VALUES('rebuild')",
                    [],
                );
                let remedy_text = match repair_res {
                    Ok(_) => "Repaired: Rebuilt FTS5 index successfully".to_string(),
                    Err(err) => format!("Repair failed: {err}"),
                };
                findings.push(DoctorFinding {
                    code: "FTS5_CORRUPT".into(),
                    severity: "error".into(),
                    message: msg,
                    remedy: Some(remedy_text),
                });
            } else {
                findings.push(DoctorFinding {
                    code: "FTS5_CORRUPT".into(),
                    severity: "error".into(),
                    message: msg,
                    remedy: Some("Run `statefulmemory doctor --repair` to rebuild FTS5 index".into()),
                });
            }
        }
    }

    // 4. Orphan embedding metadata check
    let orphan_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observation_embedding_meta WHERE observation_id NOT IN (SELECT id FROM observations WHERE deleted_at IS NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if orphan_count > 0 {
        let msg = format!("Found {orphan_count} orphan vector embedding metadata row(s)");
        if auto_repair {
            let deleted = conn.execute(
                "DELETE FROM observation_embedding_meta WHERE observation_id NOT IN (SELECT id FROM observations WHERE deleted_at IS NULL)",
                [],
            );
            let remedy_text = match deleted {
                Ok(n) => format!("Repaired: Removed {n} orphan embedding metadata rows"),
                Err(e) => format!("Repair failed: {e}"),
            };
            findings.push(DoctorFinding {
                code: "ORPHAN_VECTORS".into(),
                severity: "warn".into(),
                message: msg,
                remedy: Some(remedy_text),
            });
        } else {
            findings.push(DoctorFinding {
                code: "ORPHAN_VECTORS".into(),
                severity: "warn".into(),
                message: msg,
                remedy: Some("Run `statefulmemory doctor --repair` to purge orphan vectors".into()),
            });
        }
    } else {
        findings.push(DoctorFinding {
            code: "ORPHAN_VECTORS".into(),
            severity: "info".into(),
            message: "No orphan vector embeddings found".into(),
            remedy: None,
        });
    }

    let job_counts = crate::jobs::status_summary(conn).unwrap_or_default();
    let dead_jobs = job_counts.dead;
    let pending_jobs = job_counts.pending;
    findings.push(DoctorFinding {
        code: "DURABLE_JOBS".into(),
        severity: if dead_jobs > 0 { "warn" } else { "info" }.into(),
        message: format!("Durable jobs: {pending_jobs} pending, {dead_jobs} dead"),
        remedy: if dead_jobs > 0 {
            Some("Inspect dead jobs and requeue after correcting provider/configuration errors".into())
        } else {
            None
        },
    });

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_write;
    use tempfile::TempDir;

    fn seed_observation(conn: &Connection, id: i64, deleted: bool) {
        conn.execute(
            "INSERT INTO observations
                (id, sync_id, session_id, type, title, content, scope, deleted_at)
             VALUES (?1, ?2, 's1', 'note', ?3, ?4, 'project', ?5)",
            rusqlite::params![
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
    fn projection_health_snapshot_reuses_existing_projections() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("health.db")).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        seed_observation(&conn, 1, false);
        seed_observation(&conn, 2, true);
        conn.execute(
            "INSERT INTO facts (obs_id, subject, predicate, object, extracted_by)
             VALUES (1, 's', 'p', 'o', 'test')",
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
            "INSERT INTO entities (kind, name, norm_name) VALUES ('concept', 'alpha', 'alpha')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_mentions (entity_id, observation_id, source)
             VALUES (1, 1, 'token')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (kind, project_name, payload, status)
             VALUES ('embed', 'p', '{}', 'pending')",
            [],
        )
        .unwrap();

        let snapshot = projection_health_snapshot(&conn).unwrap();
        assert_eq!(snapshot.database_integrity, "ok");
        assert_eq!(snapshot.total_observations, 2);
        assert_eq!(snapshot.active_observations, 1);
        assert_eq!(snapshot.soft_deleted_observations, 1);
        assert_eq!(snapshot.fts_rows, 2);
        assert_eq!(snapshot.fts_missing, 0);
        assert_eq!(snapshot.fts_stale, 0);
        assert_eq!(snapshot.embedding_rows, 1);
        assert_eq!(snapshot.embedding_missing, 0);
        assert_eq!(snapshot.orphan_embeddings, 0);
        assert_eq!(snapshot.fact_rows, 1);
        assert_eq!(snapshot.fact_observations, 1);
        assert_eq!(snapshot.anchor_rows, 1);
        assert_eq!(snapshot.anchored_observations, 1);
        assert_eq!(snapshot.graph.covered_observations, 1);
        assert_eq!(snapshot.jobs.pending, 1);
        assert!(snapshot.is_healthy());
        assert_eq!(snapshot.drift(), 0);
        assert_eq!(snapshot.verify_states, vec![("unanchored".to_string(), 2)]);
    }

    #[test]
    fn project_health_snapshot_adds_project_identity() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("health.db")).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        seed_observation(&conn, 1, false);
        let snapshot = project_health_snapshot(&conn, "demo").unwrap();
        assert_eq!(snapshot.project, "demo");
        assert_eq!(snapshot.stats.project, "demo");
        assert_eq!(snapshot.stats.observations, 1);
        assert_eq!(snapshot.projections.active_observations, 1);
    }

    #[test]
    fn health_snapshot_reports_dead_jobs() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("health.db")).unwrap();
        conn.execute(
            "INSERT INTO jobs (kind, project_name, payload, status)
             VALUES ('extract', 'p', '{}', 'dead')",
            [],
        )
        .unwrap();
        let snapshot = health_snapshot(&conn).unwrap();
        assert_eq!(snapshot.jobs.dead, 1);
        assert!(!snapshot.is_healthy());
    }
}
