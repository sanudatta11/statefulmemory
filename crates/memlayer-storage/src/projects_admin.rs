//! Project administration helpers — list, merge, consolidate, prune.
//!
//! Spec sections: FR12.10–FR12.14, EC-4, OQ-3.
//!
//! These helpers operate **across** project DBs. The `merge_projects` cross-DB
//! transaction uses `ATTACH DATABASE` to wrap source + target writes in one
//! atomic unit, satisfying FR12.12 ("Atomic (transaction)").
//!
//! Similarity for `consolidate` is `strsim::jaro_winkler ≥ 0.85` per OQ-3.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use strsim::jaro_winkler;

use memlayer_core::error::{Error, Result};
use memlayer_core::paths;

/// Per-project counts surfaced by `ListProjects` (FR12.10).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCounts {
    pub normalized_name: String,
    pub display_name: String,
    pub observation_count: i64,
    pub session_count: i64,
    pub prompt_count: i64,
    pub created_at: String,
}

/// Result of `merge_projects`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub observations_migrated: i64,
    pub sessions_migrated: i64,
    pub prompts_migrated: i64,
}

/// One pair returned by `consolidate_candidates`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidateCandidate {
    pub from: String,
    pub to: String,
    pub similarity: f64,
}

/// List every project on disk together with its row counts. Active rows only
/// (`deleted_at IS NULL` for observations).
pub fn list_projects_with_counts() -> Result<Vec<ProjectCounts>> {
    list_projects_with_counts_at(&paths::projects_dir())
}

/// Same as [`list_projects_with_counts`] but takes an explicit `projects_dir`
/// instead of reading `MEMLAYER_DATA_DIR`. The public API delegates here; tests
/// drive this variant directly so they don't depend on process-global env
/// state.
pub fn list_projects_with_counts_at(projects_dir: &Path) -> Result<Vec<ProjectCounts>> {
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(projects_dir)
        .map_err(|e| Error::internal(format!("read_dir {}: {e}", projects_dir.display())))?
    {
        let entry = entry.map_err(|e| Error::internal(format!("read_dir entry: {e}")))?;
        let ft = entry
            .file_type()
            .map_err(|e| Error::internal(format!("file_type: {e}")))?;
        if !ft.is_dir() {
            continue;
        }
        let normalized = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if memlayer_core::project::validate(&normalized).is_err() {
            continue;
        }
        let cfg_path = entry.path().join("config.json");
        if !cfg_path.exists() {
            continue;
        }
        let bytes = std::fs::read(&cfg_path)
            .map_err(|e| Error::internal(format!("read {}: {e}", cfg_path.display())))?;
        let cfg: crate::registry::ProjectConfig = serde_json::from_slice(&bytes)
            .map_err(|e| Error::internal(format!("parse {}: {e}", cfg_path.display())))?;
        let db_path = projects_dir.join(format!("{normalized}.db"));
        if !db_path.exists() {
            continue;
        }
        let conn = crate::db::open_read(&db_path)?;
        out.push(ProjectCounts {
            display_name: cfg.project_display_name,
            observation_count: count(
                &conn,
                "SELECT count(*) FROM observations WHERE deleted_at IS NULL",
            )?,
            session_count: count(&conn, "SELECT count(*) FROM sessions")?,
            prompt_count: count(&conn, "SELECT count(*) FROM user_prompts")?,
            created_at: cfg.created_at,
            normalized_name: normalized,
        });
    }
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}

fn count(conn: &Connection, sql: &str) -> Result<i64> {
    conn.query_row(sql, [], |r| r.get(0))
        .map_err(|e| Error::internal(format!("count: {e}")))
}

/// Merge `from_db` into `to_db` atomically (FR12.12).
///
/// Strategy:
/// - Open a write connection on `to_db`.
/// - `ATTACH DATABASE 'from_db' AS src`.
/// - In a single `BEGIN IMMEDIATE` transaction:
///   - Copy sessions / observations / prompts via `INSERT OR IGNORE`. The FTS5
///     trigger fans new observation rows into `observations_fts` automatically.
///   - Soft-delete every observation in `src` (set `deleted_at = now()`).
///   - Sessions and prompts in `src` are left as-is — they have no
///     `deleted_at` column. The CLI follows up with `DeleteProject` to remove
///     the source DB file when the user wants the data fully gone.
/// - `DETACH` and `COMMIT`.
///
/// `EC-4`: same path for `from` and `to` is rejected as `INVALID_ARGUMENT`.
///
/// **Note:** this opens its own write connection on `to_db`. In the daemon
/// path you should call [`merge_into_target_conn`] instead, routed through
/// the target project's `WriteRequest::Custom` so the merge serializes with
/// other writes to the same DB. This standalone form is retained for tests
/// and for use cases (e.g., consolidate scripts) that don't share a write
/// thread with the target.
pub fn merge_projects(from_db: &Path, to_db: &Path) -> Result<MergeOutcome> {
    if from_db == to_db {
        return Err(Error::invalid("cannot merge a project into itself"));
    }
    if !from_db.exists() {
        return Err(Error::not_found(format!(
            "source DB does not exist: {}",
            from_db.display()
        )));
    }
    if !to_db.exists() {
        return Err(Error::not_found(format!(
            "target DB does not exist: {}",
            to_db.display()
        )));
    }
    let mut conn = crate::db::open_write(to_db)?;
    merge_into_target_conn(&mut conn, from_db)
}

/// Merge `from_db` into the target write connection that the caller has
/// already opened (typically the project registry's dedicated write thread).
/// Routing through this entry point preserves the single-writer invariant
/// — the merge tx runs on the same connection that handles SaveObservation
/// et al., so SQLite serializes them naturally.
///
/// Pre-conditions: `from_db` exists, `conn` is a write-capable connection
/// for the target DB. `from_db == to_db` rejection is the caller's
/// responsibility (the connection knows nothing of its on-disk path).
pub fn merge_into_target_conn(conn: &mut Connection, from_db: &Path) -> Result<MergeOutcome> {
    if !from_db.exists() {
        return Err(Error::not_found(format!(
            "source DB does not exist: {}",
            from_db.display()
        )));
    }

    // ATTACH outside the transaction; SQLite forbids ATTACH inside one.
    conn.execute(
        "ATTACH DATABASE ?1 AS src",
        params![from_db.to_string_lossy().to_string()],
    )
    .map_err(|e| Error::internal(format!("attach src: {e}")))?;

    let result = (|| -> Result<MergeOutcome> {
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| Error::internal(format!("BEGIN IMMEDIATE: {e}")))?;

        let sessions =
            tx.execute(
                "INSERT OR IGNORE INTO main.sessions
                    (id, directory, started_at, ended_at, summary)
                  SELECT id, directory, started_at, ended_at, summary
                    FROM src.sessions",
                [],
            )
            .map_err(|e| Error::internal(format!("copy sessions: {e}")))? as i64;

        let observations =
            tx.execute(
                "INSERT OR IGNORE INTO main.observations
                    (sync_id, session_id, type, title, content, tool_name, scope,
                     created_by, topic_key, normalized_hash, revision_count,
                     duplicate_count, last_seen_at, created_at, updated_at,
                     deleted_at, review_after)
                  SELECT sync_id, session_id, type, title, content, tool_name, scope,
                         created_by, topic_key, normalized_hash, revision_count,
                         duplicate_count, last_seen_at, created_at, updated_at,
                         deleted_at, review_after
                    FROM src.observations",
                [],
            )
            .map_err(|e| Error::internal(format!("copy observations: {e}")))? as i64;

        let prompts =
            tx.execute(
                "INSERT OR IGNORE INTO main.user_prompts
                    (sync_id, session_id, content, created_at)
                  SELECT sync_id, session_id, content, created_at
                    FROM src.user_prompts",
                [],
            )
            .map_err(|e| Error::internal(format!("copy prompts: {e}")))? as i64;

        // Soft-delete in source. Sessions and prompts have no deleted_at
        // column; only observations participate in soft-delete semantics.
        tx.execute(
            "UPDATE src.observations
                SET deleted_at = datetime('now')
              WHERE deleted_at IS NULL",
            [],
        )
        .map_err(|e| Error::internal(format!("soft-delete src observations: {e}")))?;

        tx.commit()
            .map_err(|e| Error::internal(format!("COMMIT merge: {e}")))?;

        Ok(MergeOutcome {
            observations_migrated: observations,
            sessions_migrated: sessions,
            prompts_migrated: prompts,
        })
    })();

    let _ = conn.execute("DETACH DATABASE src", []);
    result
}

/// Find pairs of projects whose normalized names look like duplicates.
///
/// Returns candidates with `similarity >= threshold` (default 0.85, OQ-3).
/// Pair direction: the project with **fewer** active observations is `from`,
/// the one with more is `to`. Ties resolved by older `created_at`.
pub fn consolidate_candidates(threshold: f64) -> Result<Vec<ConsolidateCandidate>> {
    let projects = list_projects_with_counts()?;
    Ok(consolidate_pairs(&projects, threshold))
}

/// Pure pairing logic separated from filesystem IO so tests can exercise it
/// without touching `MEMLAYER_DATA_DIR` or the project registry.
pub fn consolidate_pairs(projects: &[ProjectCounts], threshold: f64) -> Vec<ConsolidateCandidate> {
    let mut out = Vec::new();
    let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
    for (i, a) in projects.iter().enumerate() {
        for b in &projects[i + 1..] {
            let sim = jaro_winkler(&a.normalized_name, &b.normalized_name);
            if sim < threshold {
                continue;
            }
            let (from, to) = match a.observation_count.cmp(&b.observation_count) {
                std::cmp::Ordering::Less => (a, b),
                std::cmp::Ordering::Greater => (b, a),
                std::cmp::Ordering::Equal => {
                    if a.created_at <= b.created_at {
                        (b, a)
                    } else {
                        (a, b)
                    }
                }
            };
            let key = (from.normalized_name.clone(), to.normalized_name.clone());
            if seen_pairs.insert(key) {
                out.push(ConsolidateCandidate {
                    from: from.normalized_name.clone(),
                    to: to.normalized_name.clone(),
                    similarity: sim,
                });
            }
        }
    }
    // Most-similar first.
    out.sort_by(|x, y| {
        y.similarity
            .partial_cmp(&x.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// List projects with zero active observations (FR12.14, prune candidates).
pub fn prune_candidates() -> Result<Vec<String>> {
    prune_candidates_at(&paths::projects_dir())
}

/// Test-friendly variant of [`prune_candidates`] that takes an explicit
/// `projects_dir`.
pub fn prune_candidates_at(projects_dir: &Path) -> Result<Vec<String>> {
    let projects = list_projects_with_counts_at(projects_dir)?;
    Ok(projects
        .into_iter()
        .filter(|p| p.observation_count == 0)
        .map(|p| p.normalized_name)
        .collect())
}

/// Hard-delete a project: remove its DB file and per-project config dir.
///
/// Soft-delete of all rows is handled by passing `hard=false` from the CLI
/// — it walks the project DB and sets `deleted_at` on observations. The
/// hard variant is what this helper does.
pub fn hard_delete_project(normalized: &str) -> Result<()> {
    hard_delete_project_at(&paths::projects_dir(), normalized)
}

/// Test-friendly variant of [`hard_delete_project`].
pub fn hard_delete_project_at(projects_dir: &Path, normalized: &str) -> Result<()> {
    let db = projects_dir.join(format!("{normalized}.db"));
    let dir = projects_dir.join(normalized);
    if db.exists() {
        std::fs::remove_file(&db)
            .map_err(|e| Error::internal(format!("rm {}: {e}", db.display())))?;
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| Error::internal(format!("rm -r {}: {e}", dir.display())))?;
    }
    Ok(())
}

/// Soft-delete every active observation in a project. Used by
/// `DeleteProject` when `hard = false`.
pub fn soft_delete_project_observations(db: &Path) -> Result<i64> {
    if !db.exists() {
        return Ok(0);
    }
    let conn = crate::db::open_write(db)?;
    let n = conn
        .execute(
            "UPDATE observations SET deleted_at = datetime('now') WHERE deleted_at IS NULL",
            [],
        )
        .map_err(|e| Error::internal(format!("soft-delete project: {e}")))? as i64;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ProjectConfig;
    use rusqlite::params;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Materialize a project on disk under an explicit `data_dir`. No env vars.
    fn make_project(data_dir: &Path, display_name: &str) -> PathBuf {
        let normalized = memlayer_core::project::normalize(display_name).unwrap();
        let dir = data_dir.join("projects").join(&normalized);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = ProjectConfig {
            project_display_name: display_name.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            repo_path: None,
        };
        std::fs::write(
            dir.join("config.json"),
            serde_json::to_vec_pretty(&cfg).unwrap(),
        )
        .unwrap();
        let db_path = data_dir.join("projects").join(format!("{normalized}.db"));
        let conn = crate::db::open_write(&db_path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        db_path
    }

    fn insert_obs(db_path: &Path, sync_id: &str, content: &str) {
        let conn = crate::db::open_write(db_path).unwrap();
        conn.execute(
            "INSERT INTO observations
                (sync_id, session_id, type, title, content, scope)
              VALUES (?1, 's1', 'note', 't', ?2, 'project')",
            params![sync_id, content],
        )
        .unwrap();
    }

    /// Returns `<tempdir>/projects` after creating the parent. Tests pass
    /// this path into the `*_at` variants so they never touch
    /// `MEMLAYER_DATA_DIR` (which is a process-global the rest of the test
    /// suite already serializes through `registry::tests::ENV_LOCK`).
    fn fresh_projects_dir(td: &TempDir) -> PathBuf {
        let p = td.path().join("projects");
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn merge_moves_rows_atomically() {
        let td = TempDir::new().unwrap();
        let from = make_project(td.path(), "alpha");
        let to = make_project(td.path(), "beta");
        // Both projects start with session 's1' from make_project. Add a
        // unique session in `from` so we can verify it migrates while the
        // duplicate 's1' is correctly skipped (INSERT OR IGNORE).
        {
            let conn = crate::db::open_write(&from).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, directory) VALUES ('only-in-src', '/tmp')",
                [],
            )
            .unwrap();
        }
        insert_obs(&from, "obs-1", "first body");
        insert_obs(&from, "obs-2", "second body");

        let outcome = merge_projects(&from, &to).unwrap();
        assert_eq!(outcome.observations_migrated, 2);
        // 's1' is in both DBs (skipped via INSERT OR IGNORE); 'only-in-src'
        // is unique to from and migrates over.
        assert_eq!(outcome.sessions_migrated, 1);

        // Target now has both observation rows.
        let to_conn = crate::db::open_read(&to).unwrap();
        let to_count: i64 = to_conn
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(to_count, 2);
        // And it now has the unique session too.
        let has_unique: i64 = to_conn
            .query_row(
                "SELECT count(*) FROM sessions WHERE id = 'only-in-src'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_unique, 1);

        // Source observations are soft-deleted.
        let from_conn = crate::db::open_read(&from).unwrap();
        let active_in_src: i64 = from_conn
            .query_row(
                "SELECT count(*) FROM observations WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            active_in_src, 0,
            "source observations should be soft-deleted"
        );
    }

    #[test]
    fn merge_self_to_self_invalid_argument() {
        let td = TempDir::new().unwrap();
        let p = make_project(td.path(), "alpha");
        let r = merge_projects(&p, &p);
        assert!(matches!(r, Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn merge_into_missing_target_returns_not_found() {
        let td = TempDir::new().unwrap();
        let from = make_project(td.path(), "alpha");
        let bogus = td.path().join("projects").join("nope.db");
        let r = merge_projects(&from, &bogus);
        assert!(matches!(r, Err(Error::NotFound(_))));
    }

    fn pc(name: &str, observation_count: i64) -> ProjectCounts {
        ProjectCounts {
            normalized_name: name.to_string(),
            display_name: name.to_string(),
            observation_count,
            session_count: 0,
            prompt_count: 0,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn consolidate_uses_jaro_winkler() {
        // Pure function — no filesystem coupling, so the test stays
        // deterministic regardless of MEMLAYER_DATA_DIR / parallel test order.
        let projects = vec![
            pc("memlayer", 3),
            pc("memlayer-cli", 0),
            pc("totally-other", 1),
        ];
        let pairs = consolidate_pairs(&projects, 0.85);

        // memlayer ~ memlayer-cli must be flagged. memlayer ~ totally-other
        // is well below 0.85 and must not appear.
        assert!(
            pairs.iter().any(|c| {
                let names = (c.from.as_str(), c.to.as_str());
                names == ("memlayer-cli", "memlayer") || names == ("memlayer", "memlayer-cli")
            }),
            "expected memlayer/memlayer-cli pair, got {pairs:?}"
        );
        assert!(
            !pairs
                .iter()
                .any(|c| c.from.contains("totally") || c.to.contains("totally")),
            "totally-other must not appear: {pairs:?}"
        );
    }

    #[test]
    fn consolidate_orders_smaller_to_larger() {
        let projects = vec![pc("memlayer", 3), pc("memlayerr", 1)];
        let pairs = consolidate_pairs(&projects, 0.85);
        let p = pairs
            .iter()
            .find(|c| {
                (c.from == "memlayerr" && c.to == "memlayer")
                    || (c.from == "memlayer" && c.to == "memlayerr")
            })
            .expect("pair present");
        // Smaller (memlayerr, 1 obs) → from; larger (memlayer, 3) → to.
        assert_eq!(p.from, "memlayerr");
        assert_eq!(p.to, "memlayer");
    }

    #[test]
    fn consolidate_threshold_filters_dissimilar_pairs() {
        let projects = vec![pc("alpha", 1), pc("zulu", 1)];
        let pairs = consolidate_pairs(&projects, 0.85);
        assert!(pairs.is_empty(), "alpha vs zulu should be < 0.85");
    }

    #[test]
    fn prune_excludes_projects_with_obs() {
        let td = TempDir::new().unwrap();
        let a = make_project(td.path(), "alpha");
        let _b = make_project(td.path(), "beta-empty");
        let _c = make_project(td.path(), "gamma-empty");
        insert_obs(&a, "o1", "stay");

        let mut candidates = prune_candidates_at(&fresh_projects_dir(&td)).unwrap();
        candidates.sort();
        assert_eq!(
            candidates,
            vec!["beta-empty".to_string(), "gamma-empty".to_string()]
        );
    }

    #[test]
    fn prune_treats_soft_deleted_as_empty() {
        let td = TempDir::new().unwrap();
        let a = make_project(td.path(), "alpha");
        insert_obs(&a, "o1", "x");
        // Soft-delete the lone row.
        let conn = crate::db::open_write(&a).unwrap();
        conn.execute("UPDATE observations SET deleted_at = datetime('now')", [])
            .unwrap();

        let candidates = prune_candidates_at(&fresh_projects_dir(&td)).unwrap();
        assert_eq!(candidates, vec!["alpha".to_string()]);
    }

    #[test]
    fn list_projects_returns_counts() {
        let td = TempDir::new().unwrap();
        let a = make_project(td.path(), "alpha");
        insert_obs(&a, "o1", "x");
        insert_obs(&a, "o2", "y");

        let list = list_projects_with_counts_at(&fresh_projects_dir(&td)).unwrap();
        let alpha = list.iter().find(|p| p.normalized_name == "alpha").unwrap();
        assert_eq!(alpha.observation_count, 2);
        assert_eq!(alpha.session_count, 1);
        assert_eq!(alpha.display_name, "alpha");
    }

    #[test]
    fn hard_delete_removes_db_and_dir() {
        let td = TempDir::new().unwrap();
        let p = make_project(td.path(), "alpha");
        let projects_dir = fresh_projects_dir(&td);
        assert!(p.exists());
        hard_delete_project_at(&projects_dir, "alpha").unwrap();
        assert!(!p.exists(), "DB file should be gone");
        assert!(
            !projects_dir.join("alpha").exists(),
            "project dir should be gone"
        );
    }

    #[test]
    fn soft_delete_project_observations_zeros_active_count() {
        let td = TempDir::new().unwrap();
        let a = make_project(td.path(), "alpha");
        insert_obs(&a, "o1", "x");
        insert_obs(&a, "o2", "y");
        let n = soft_delete_project_observations(&a).unwrap();
        assert_eq!(n, 2);
    }
}
