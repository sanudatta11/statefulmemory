//! DB open / migrate / close.

use std::path::Path;

use rusqlite::Connection;
use tracing::debug;

use memlayer_core::error::{Error, Result};

use crate::pragmas;

/// Open a writeable connection, run migrations, and apply pragmas.
///
/// Creates the parent directory if it doesn't exist. Caller is responsible for
/// holding this connection on the dedicated write thread.
pub fn open_write(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open_with_flags(path, pragmas::write_flags())
        .map_err(|e| Error::internal(format!("open_write({}): {e}", path.display())))?;
    pragmas::apply(&conn)?;
    crate::run_migrations(&mut conn)?;
    debug!(path = %path.display(), "opened write connection");
    Ok(conn)
}

/// Open a read-only connection on an existing DB file.
///
/// The caller must have already opened a write connection at least once so
/// migrations and the file itself exist.
pub fn open_read(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, pragmas::read_flags())
        .map_err(|e| Error::internal(format!("open_read({}): {e}", path.display())))?;
    pragmas::apply(&conn)?;
    Ok(conn)
}

/// Marker trait satisfied by any `Connection`-providing object.
///
/// Used by callers (e.g., dedupe) that don't care whether the connection is
/// read or write.
pub trait Migrate {
    fn conn(&self) -> &Connection;
}

impl Migrate for Connection {
    fn conn(&self) -> &Connection {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn migrate_creates_schema() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_write(&path).unwrap();
        let v: String = conn
            .query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "1");
    }

    #[test]
    fn migrate_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        // Open + migrate twice — second open must succeed.
        drop(open_write(&path).unwrap());
        drop(open_write(&path).unwrap());
    }

    #[test]
    fn observations_fts_writes_via_trigger() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_write(&path).unwrap();

        // Insert a session first (FK constraint).
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO observations (sync_id, session_id, type, title, content) \
             VALUES ('sync-1', 's1', 'note', 'hello world', 'unique-content-xyz')",
            [],
        )
        .unwrap();

        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM observations_fts WHERE observations_fts MATCH '\"unique-content-xyz\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
