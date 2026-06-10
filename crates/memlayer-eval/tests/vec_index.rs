// Generated with AI Coding Rules Hub
//! Integration test for spec-task-10: sqlite-vec lifecycle + sidecar bootstrap.
//!
//! Covers EH-1 / SC-10 happy path:
//!   1. `open_with_vec` produces a Connection with `vec0` registered.
//!   2. `bootstrap_vec_index` creates `obs_vec` (vec0) and `obs_id_map`
//!      idempotently.
//!   3. A 384-dim float vector inserts via the byte-stream encoding the
//!      hybrid retrieval path will use in spec-task-11.
//!   4. `vec_version()` returns a non-empty string (extension actually
//!      registered, not silently no-oped).
//!
//! Uses `tempfile::TempDir` for isolation — no env-var manipulation, so
//! parallel `cargo test` execution is safe (no `--test-threads=1` required).

use memlayer_eval::vec_index::{bootstrap_vec_index, open_with_vec};
use rusqlite::params;
use tempfile::TempDir;

/// Encode a `&[f32]` to little-endian bytes the way `vec0` expects on insert.
/// This mirrors the pattern spec-task-11 will use inside `retrieve_hybrid`.
fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[test]
fn open_with_vec_loads_extension_and_bootstraps_idempotently() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("vec").join("project.vec.db");

    // First open: parent dir must be created on the fly.
    let conn = open_with_vec(&db_path).expect("open_with_vec");

    // vec_version() proves the extension actually registered, not that load
    // silently no-oped.
    let version: String = conn
        .query_row("SELECT vec_version()", [], |row| row.get(0))
        .expect("vec_version()");
    assert!(!version.is_empty(), "vec_version() returned empty string");

    // Bootstrap creates obs_vec + obs_id_map.
    bootstrap_vec_index(&conn, "obs", 384).expect("bootstrap_vec_index");

    // Idempotent: second call must not error.
    bootstrap_vec_index(&conn, "obs", 384).expect("bootstrap_vec_index (2nd)");

    // Insert one row using the byte-stream encoding hybrid retrieval will use.
    let embedding: Vec<f32> = (0..384).map(|i| (i as f32) * 1e-3).collect();
    let bytes = vec_to_bytes(&embedding);
    conn.execute(
        "INSERT INTO obs_vec(rowid, embedding) VALUES (1, ?)",
        params![bytes],
    )
    .expect("insert into obs_vec");

    // id_map insert reflects the eventual rowid -> observations.id link.
    conn.execute(
        "INSERT INTO obs_id_map(vec_rowid, observation_id) VALUES (1, 42)",
        [],
    )
    .expect("insert into obs_id_map");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM obs_vec", [], |row| row.get(0))
        .expect("count obs_vec");
    assert_eq!(count, 1, "obs_vec should have exactly one row");

    let mapped_obs: i64 = conn
        .query_row(
            "SELECT observation_id FROM obs_id_map WHERE vec_rowid = 1",
            [],
            |row| row.get(0),
        )
        .expect("query obs_id_map");
    assert_eq!(mapped_obs, 42);
}

#[test]
fn open_with_vec_reopens_existing_db() {
    // SC-10 implication: re-opening the sidecar must not corrupt or duplicate
    // schema. This also exercises the `Once`-guarded init across two opens.
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("project.vec.db");

    {
        let conn = open_with_vec(&db_path).expect("first open");
        bootstrap_vec_index(&conn, "obs", 384).expect("bootstrap");
    }

    let conn = open_with_vec(&db_path).expect("second open");
    // Bootstrap again — must remain a no-op.
    bootstrap_vec_index(&conn, "obs", 384).expect("re-bootstrap");

    let version: String = conn
        .query_row("SELECT vec_version()", [], |row| row.get(0))
        .expect("vec_version() after reopen");
    assert!(!version.is_empty());
}
