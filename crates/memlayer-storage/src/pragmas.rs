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
    ("mmap_size", "268435456"),     // 256 MB
    ("cache_size", "-65536"),        // 64 MB (negative = KiB)
    ("busy_timeout", "5000"),        // 5s
];

/// Apply the standard pragmas to `conn`.
///
/// Notes:
/// - `journal_mode = WAL` is special: the result of the assignment is queryable.
///   We use `query_row` for that one and `pragma_update` for the rest.
pub fn apply(conn: &Connection) -> Result<()> {
    for (key, value) in PRAGMAS {
        if *key == "journal_mode" {
            // SQLite returns the new mode from `PRAGMA journal_mode = WAL` — fetch it
            // so a misconfigured DB (read-only mount, etc.) surfaces as an error.
            let mode: String = conn
                .query_row(&format!("PRAGMA journal_mode = {value}"), [], |row| row.get(0))
                .map_err(|e| Error::internal(format!("pragma journal_mode: {e}")))?;
            if !mode.eq_ignore_ascii_case(value) {
                return Err(Error::internal(format!(
                    "journal_mode could not be set to {value} (got {mode})"
                )));
            }
        } else {
            conn.pragma_update(None, key, value)
                .map_err(|e| Error::internal(format!("pragma {key}: {e}")))?;
        }
    }
    Ok(())
}

/// Standard flags for a writeable connection.
pub fn write_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// Standard flags for a read-only connection.
pub fn read_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
}
