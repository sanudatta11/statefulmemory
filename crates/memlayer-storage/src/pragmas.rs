//! Pragma application — invoked on every fresh connection (read or write).
//!
//! Source: PRD §5.2. The values are normative.

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
            let _ = conn
                .query_row(&format!("PRAGMA {key} = {value}"), [], |_| Ok(()))
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
