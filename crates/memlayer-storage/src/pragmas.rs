//! Pragma application — invoked on every fresh connection (read or write).
//!
//! Source: PRD §5.2. The values are normative.

use std::sync::Once;

use rusqlite::{Connection, OpenFlags};

use memlayer_core::error::{Error, Result};

const PRAGMAS: &[(&str, &str)] = &[
    ("journal_mode", "WAL"),
    ("synchronous", "NORMAL"),
    ("foreign_keys", "ON"),
    ("temp_store", "MEMORY"),
    ("mmap_size", "268435456"), // 256 MB
    ("cache_size", "-65536"),   // 64 MB (negative = KiB)
    ("busy_timeout", "5000"),   // 5s
];

/// Stored model id used in `observation_embedding_meta.model`. Matches the
/// `BAAI/bge-small-en-v1.5` model id used by `memlayer-embed`, sans HF org
/// prefix (storage stores the bare model name; the registry/embedder uses the
/// full HF id when downloading).
pub const STORED_EMBED_MODEL: &str = "bge-small-en-v1.5";

/// Apply the standard pragmas to `conn`.
///
/// Notes:
/// - `journal_mode = WAL` returns the new mode; we verify it was applied.
/// - `mmap_size` and `cache_size` also return rows on assignment.
/// - rusqlite's `execute_batch` with `bundled-full` enables the `extra_check`
///   feature, which makes it reject any statement that produces rows. We use
///   `query_row` unconditionally so row-returning pragmas are handled safely.
pub fn apply(conn: &Connection) -> Result<()> {
    for (key, value) in PRAGMAS {
        if *key == "journal_mode" {
            let mode: String = conn
                .query_row(&format!("PRAGMA journal_mode = {value}"), [], |row| {
                    row.get(0)
                })
                .map_err(|e| Error::internal(format!("pragma journal_mode: {e}")))?;
            if !mode.eq_ignore_ascii_case(value) {
                return Err(Error::internal(format!(
                    "journal_mode could not be set to {value} (got {mode})"
                )));
            }
        } else {
            // Use query_row to safely handle pragmas that may return a result row.
            // We ignore the returned value for all non-journal_mode pragmas.
            conn.query_row(&format!("PRAGMA {key} = {value}"), [], |_| Ok(()))
                .or_else(|e| match e {
                    // QueryReturnedNoRows is fine — pragma had no result row.
                    rusqlite::Error::QueryReturnedNoRows => Ok(()),
                    other => Err(other),
                })
                .map_err(|e| Error::internal(format!("pragma {key}: {e}")))?;
        }
    }
    Ok(())
}

/// Standard flags for a writeable connection.
pub fn write_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// Standard flags for a read-only connection.
pub fn read_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// Process-wide guard for `sqlite-vec` auto-extension registration. The
/// `sqlite-vec` 0.1.x crate exposes only `sqlite3_vec_init`, registered as a
/// SQLite auto-extension. Once registered, every `Connection::open` thereafter
/// gets `vec0` for free; per-connection enable/load/disable is neither
/// necessary nor supported on this crate version.
static VEC_INIT: Once = Once::new();

/// Register the `sqlite-vec` extension as a process-global SQLite
/// auto-extension. Idempotent — only runs once per process regardless of how
/// many threads race to invoke it. Pattern copied from `memlayer-eval/src/vec_index.rs`.
pub fn ensure_sqlite_vec_extension() {
    VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is an `extern "C"` symbol from the
        // sqlite-vec C library with the exact signature SQLite expects for an
        // extension entrypoint.
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

/// Verify that the `sqlite-vec` extension is loaded into this connection by
/// calling its `vec_version()` scalar. Returns the version string on success,
/// `Err(FailedPrecondition)` otherwise. Callers that need a hard guarantee at
/// open-time (write connections that may run V4 migration) use this; read
/// connections rely on the auto-extension having been registered earlier in
/// the process.
pub fn smoke_test_sqlite_vec(conn: &Connection) -> Result<String> {
    conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0))
        .map_err(|e| {
            Error::FailedPrecondition(format!(
                "sqlite-vec extension not loaded: {e}; call \
                 pragmas::ensure_sqlite_vec_extension() before opening"
            ))
        })
}

/// Verify the daemon's configured embed model matches what's stored in this
/// project DB (SC-11). On mismatch returns `FailedPrecondition` with a hint
/// pointing at `memlayer reindex`. An empty meta table (no embeddings yet)
/// is fine — anything we embed from now on will use the configured model.
pub fn check_embed_model_compat(conn: &Connection, configured: &str) -> Result<()> {
    // V4 may not have run yet on legacy DBs (a fresh open_write runs it
    // first; but read-only callers might race). If the table is missing,
    // there are zero embeddings stored, so there's nothing to check.
    let table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master \
             WHERE type='table' AND name='observation_embedding_meta'",
            [],
            |_| Ok(true),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(other),
        })
        .map_err(|e| Error::internal(format!("schema probe: {e}")))?;
    if !table_exists {
        return Ok(());
    }

    let stored: Option<String> = conn
        .query_row(
            "SELECT DISTINCT model FROM observation_embedding_meta LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .map_err(|e| Error::internal(format!("read stored embed model: {e}")))?;

    match stored {
        None => Ok(()),
        Some(s) if s == configured => Ok(()),
        Some(s) => Err(Error::FailedPrecondition(format!(
            "embed model mismatch: stored \"{s}\", configured \"{configured}\"; \
             run `memlayer reindex` to clear and re-embed"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_vec_extension_loads() {
        ensure_sqlite_vec_extension();
        let conn = Connection::open_in_memory().unwrap();
        let v = smoke_test_sqlite_vec(&conn).expect("vec_version() must work");
        assert!(!v.is_empty(), "vec_version returned empty string: {v:?}");
    }

    #[test]
    fn vec0_virtual_table_creatable() {
        ensure_sqlite_vec_extension();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE VIRTUAL TABLE vt USING vec0(embedding float[8])",
            [],
        )
        .expect("vec0 vtable must create");
    }

    #[test]
    fn embed_model_compat_passes_when_no_table() {
        let conn = Connection::open_in_memory().unwrap();
        check_embed_model_compat(&conn, "bge-small-en-v1.5").unwrap();
    }

    #[test]
    fn embed_model_compat_passes_when_table_empty() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE observation_embedding_meta (
                observation_id INTEGER PRIMARY KEY,
                model TEXT NOT NULL,
                dim INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                quantized INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        check_embed_model_compat(&conn, STORED_EMBED_MODEL).unwrap();
    }

    #[test]
    fn embed_model_compat_fails_on_mismatch() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE observation_embedding_meta (
                observation_id INTEGER PRIMARY KEY,
                model TEXT NOT NULL,
                dim INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                quantized INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO observation_embedding_meta(observation_id, model, dim, created_at) \
             VALUES (1, 'bge-base-en', 768, '2026-01-01')",
            [],
        )
        .unwrap();

        let err = check_embed_model_compat(&conn, "bge-small-en-v1.5")
            .expect_err("must fail on mismatch");
        let msg = format!("{err}");
        assert!(msg.contains("memlayer reindex"), "msg: {msg}");
        assert!(msg.contains("bge-base-en"), "msg: {msg}");
    }
}
