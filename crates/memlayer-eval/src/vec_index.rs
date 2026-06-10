// Generated with AI Coding Rules Hub
//! sqlite-vec extension lifecycle and sidecar DB bootstrap (spec-task-10).
//!
//! P1 hybrid retrieval requires the `vec0` virtual table provided by the
//! `sqlite-vec` loadable extension. The plan §1.2 sketched a
//! `load_extension_enable → sqlite_vec::load → load_extension_disable`
//! sequence, but `sqlite-vec` 0.1.x only ships a single FFI symbol —
//! `sqlite3_vec_init` — registered as an **auto-extension** on the bundled
//! SQLite C library. Once registered, every subsequent `Connection::open`
//! gets `vec0` for free; no per-connection enable/load/disable dance is
//! needed (or possible) on this crate version.
//!
//! Operationally:
//!   * Register `sqlite3_vec_init` via `rusqlite::ffi::sqlite3_auto_extension`
//!     exactly once per process (guarded by `Once`).
//!   * `Connection::open(path)` then auto-loads vec0.
//!   * Smoke-test with `SELECT vec_version()`.
//!
//! If any step fails we surface a hard error (**EH-1**) rather than silently
//! degrading to BM25-only — masking a configuration bug would defeat the
//! whole point of the eval harness. The remediation message points at
//! deferred work (spec §12) for the eventual `usearch-rs` fallback.
//!
//! `bootstrap_vec_index` is idempotent: every call uses
//! `CREATE … IF NOT EXISTS` so re-opening a sidecar DB is safe. The schema
//! lives entirely on the eval side (`<data_dir>/vec/<project>.vec.db`) so
//! `memlayer-storage` and the project DB stay pristine (SC-10).

use std::path::Path;
use std::sync::Once;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Registers `sqlite-vec` as a SQLite auto-extension exactly once for the
/// lifetime of the process. The init call mutates global state in the bundled
/// SQLite C library, so guarding it with `Once` keeps multi-threaded callers
/// (eval runner, integration tests) safe.
static VEC_INIT: Once = Once::new();

/// Install `sqlite3_vec_init` as a SQLite auto-extension. Idempotent — only
/// runs once per process regardless of how many `open_with_vec` callers race
/// to invoke it.
fn ensure_vec_auto_extension() {
    VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is a `extern "C"` symbol from the
        // `sqlite-vec` C library shipped by the `sqlite-vec` crate, with the
        // exact signature SQLite expects for an extension entrypoint. Casting
        // it through `*const ()` to the FFI's expected pointer type is the
        // pattern documented in the `sqlite-vec` crate itself.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const ()
            )));
        }
    });
}

/// Open (or create) a SQLite database at `path`, ensure the `sqlite-vec`
/// extension is registered, and return a connection ready for `vec0` virtual
/// tables.
///
/// **EH-1:** any failure — registering the auto-extension, opening the DB,
/// or the `vec_version()` smoke test — returns a hard error with a
/// remediation message. We do NOT silently fall back to BM25-only.
pub fn open_with_vec(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create parent directory for vec sidecar: {}", parent.display())
            })?;
        }
    }

    // Register the auto-extension before opening the connection so vec0 is
    // available the moment `Connection::open` returns.
    ensure_vec_auto_extension();

    let conn = Connection::open(path)
        .with_context(|| format!("open sqlite db at {}", path.display()))?;

    // Smoke-test that vec0 actually registered. `vec_version()` is a scalar
    // function added by the extension; if it's missing the auto-extension
    // silently failed and we surface EH-1 here.
    let version: String = conn
        .query_row("SELECT vec_version()", [], |row| row.get(0))
        .map_err(|e| {
            anyhow::anyhow!(
                "sqlite-vec extension failed to load: {e}. \
                 The vec0 virtual table is required for hybrid retrieval. \
                 See deferred work in spec §12 for the usearch-rs fallback."
            )
        })?;
    tracing::debug!(
        target: "memlayer_eval::vec_index",
        version = %version,
        path = %path.display(),
        "sqlite-vec extension loaded"
    );

    Ok(conn)
}

/// Create the `<table_prefix>_vec` virtual table (`vec0` module) and the
/// `<table_prefix>_id_map` mapping table on an already-opened connection.
///
/// Idempotent — both statements use `IF NOT EXISTS`, so callers can invoke
/// this on every open without conditionals.
///
/// `dim` should be 384 for BGE-small (P1's chosen embedding model). The
/// dimensionality is baked into the virtual table schema; changing it later
/// requires dropping and rebuilding the sidecar.
///
/// SQLite's virtual-table syntax does not accept bind parameters for table
/// names or column dimensions, so we interpolate via `format!`. Callers must
/// pass a trusted `table_prefix` (configuration, not user input).
pub fn bootstrap_vec_index(
    conn: &Connection,
    table_prefix: &str,
    dim: usize,
) -> Result<()> {
    let create_vec = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {prefix}_vec \
         USING vec0(embedding float[{dim}])",
        prefix = table_prefix,
        dim = dim,
    );
    conn.execute(&create_vec, [])
        .with_context(|| format!("create {table_prefix}_vec virtual table"))?;

    let create_id_map = format!(
        "CREATE TABLE IF NOT EXISTS {prefix}_id_map ( \
            vec_rowid      INTEGER PRIMARY KEY, \
            observation_id INTEGER NOT NULL UNIQUE \
         )",
        prefix = table_prefix,
    );
    conn.execute(&create_id_map, [])
        .with_context(|| format!("create {table_prefix}_id_map table"))?;

    Ok(())
}
