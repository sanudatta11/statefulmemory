//! Token store backed by a tiny SQLite DB (`~/.memlayer/tokens.db`).
//!
//! Schema:
//! ```sql
//! CREATE TABLE IF NOT EXISTS tokens (
//!   name       TEXT PRIMARY KEY,
//!   sha256_hex TEXT NOT NULL,
//!   is_admin   INTEGER NOT NULL DEFAULT 0,
//!   created_at TEXT NOT NULL DEFAULT (datetime('now')),
//!   revoked_at TEXT
//! );
//! ```
//!
//! The plaintext token is *never* stored. `auth.rs` looks up by SHA-256.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use subtle::ConstantTimeEq;

use memlayer_core::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct TokenMeta {
    pub name: String,
    pub hash: Vec<u8>,
    pub is_admin: bool,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

pub struct TokenStore {
    conn: Mutex<Connection>,
}

impl TokenStore {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)
            .map_err(|e| Error::internal(format!("open tokens.db: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS tokens (
                 name       TEXT PRIMARY KEY,
                 sha256_hex TEXT NOT NULL,
                 is_admin   INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 revoked_at TEXT
             )",
            [],
        )
        .map_err(|e| Error::internal(format!("create tokens table: {e}")))?;
        Ok(TokenStore {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(&self, meta: TokenMeta) -> Result<()> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        let hex = hex::encode(&meta.hash);
        conn.execute(
            "INSERT INTO tokens (name, sha256_hex, is_admin, created_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![meta.name, hex, meta.is_admin as i64, meta.created_at, meta.revoked_at],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                Error::AlreadyExists(format!("token name '{}'", meta.name))
            } else {
                Error::internal(format!("insert token: {e}"))
            }
        })?;
        Ok(())
    }

    pub fn revoke(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        let now = chrono::Utc::now().to_rfc3339();
        let n = conn
            .execute(
                "UPDATE tokens SET revoked_at = ?2 WHERE name = ?1 AND revoked_at IS NULL",
                params![name, now],
            )
            .map_err(|e| Error::internal(format!("revoke: {e}")))?;
        if n == 0 {
            return Err(Error::not_found(format!("token '{name}'")));
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<TokenMeta>> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT name, sha256_hex, is_admin, created_at, revoked_at FROM tokens ORDER BY created_at")
            .map_err(|e| Error::internal(format!("prepare list: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let hex_s: String = row.get(1)?;
                Ok(TokenMeta {
                    name: row.get(0)?,
                    hash: hex::decode(&hex_s).unwrap_or_default(),
                    is_admin: row.get::<_, i64>(2)? != 0,
                    created_at: row.get(3)?,
                    revoked_at: row.get(4)?,
                })
            })
            .map_err(|e| Error::internal(format!("list query: {e}")))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| Error::internal(format!("list rows: {e}")))?;
        Ok(rows)
    }

    /// Constant-time match by candidate hash. Returns metadata when the hash
    /// is recognized (revoked or not). Caller decides whether to honor a
    /// revoked token.
    pub fn find(&self, candidate: &[u8]) -> Option<TokenMeta> {
        let all = self.list().ok()?;
        for meta in all {
            if meta.hash.len() == candidate.len()
                && bool::from(meta.hash.as_slice().ct_eq(candidate))
            {
                return Some(meta);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    fn make_store() -> (TempDir, TokenStore) {
        let d = TempDir::new().unwrap();
        let s = TokenStore::open(d.path().join("tokens.db")).unwrap();
        (d, s)
    }

    fn h(s: &str) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(s.as_bytes());
        hasher.finalize().to_vec()
    }

    #[test]
    fn insert_then_find_round_trip() {
        let (_d, s) = make_store();
        let hash = h("abc123");
        s.insert(TokenMeta {
            name: "alice".into(),
            hash: hash.clone(),
            is_admin: true,
            created_at: chrono::Utc::now().to_rfc3339(),
            revoked_at: None,
        })
        .unwrap();
        let m = s.find(&hash).unwrap();
        assert_eq!(m.name, "alice");
        assert!(m.is_admin);
    }

    #[test]
    fn duplicate_name_already_exists() {
        let (_d, s) = make_store();
        s.insert(TokenMeta {
            name: "alice".into(),
            hash: h("a"),
            is_admin: false,
            created_at: chrono::Utc::now().to_rfc3339(),
            revoked_at: None,
        })
        .unwrap();
        let err = s
            .insert(TokenMeta {
                name: "alice".into(),
                hash: h("b"),
                is_admin: false,
                created_at: chrono::Utc::now().to_rfc3339(),
                revoked_at: None,
            })
            .err()
            .unwrap();
        assert!(matches!(err, Error::AlreadyExists(_)));
    }

    #[test]
    fn revoke_marks_revoked_at() {
        let (_d, s) = make_store();
        s.insert(TokenMeta {
            name: "x".into(),
            hash: h("z"),
            is_admin: false,
            created_at: chrono::Utc::now().to_rfc3339(),
            revoked_at: None,
        })
        .unwrap();
        s.revoke("x").unwrap();
        let v = s.list().unwrap();
        assert!(v[0].revoked_at.is_some());
    }
}
