//! Doctor auto-repair and health audit module for memlayer storage.

use memlayer_core::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorFinding {
    pub code: String,
    pub severity: String, // "info" | "warn" | "error"
    pub message: String,
    pub remedy: Option<String>,
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
                            "Run `memlayer doctor --repair` to rebuild the FTS5 index".into(),
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
                    remedy: Some("Run `memlayer doctor --repair` to rebuild FTS5 index".into()),
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
                remedy: Some("Run `memlayer doctor --repair` to purge orphan vectors".into()),
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

    Ok(findings)
}
