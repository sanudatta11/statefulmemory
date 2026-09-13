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
//! CREATE TABLE IF NOT EXISTS project_grants (
//!   project   TEXT NOT NULL,
//!   principal TEXT NOT NULL,
//!   role      TEXT NOT NULL,   -- "read" | "write"
//!   granted_at TEXT NOT NULL DEFAULT (datetime('now')),
//!   PRIMARY KEY (project, principal)
//! );
//! ```
//!
//! The plaintext token is *never* stored. `auth.rs` looks up by SHA-256.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
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
        let conn =
            Connection::open(&path).map_err(|e| Error::internal(format!("open tokens.db: {e}")))?;
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
        conn.execute(
            "CREATE TABLE IF NOT EXISTS project_grants (
                 project    TEXT NOT NULL,
                 principal  TEXT NOT NULL,
                 role       TEXT NOT NULL CHECK (role IN ('read','write')),
                 granted_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (project, principal)
             )",
            [],
        )
        .map_err(|e| Error::internal(format!("create project_grants table: {e}")))?;
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
            params![
                meta.name,
                hex,
                meta.is_admin as i64,
                meta.created_at,
                meta.revoked_at
            ],
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
        let row_iter = stmt
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
            .map_err(|e| Error::internal(format!("list query: {e}")))?;
        let mut rows = Vec::new();
        for r in row_iter {
            rows.push(r.map_err(|e| Error::internal(format!("list rows: {e}")))?);
        }
        Ok(rows)
    }

    /// Constant-time match by candidate hash. Returns metadata when the hash
    /// is recognized (revoked or not). Caller decides whether to honor a
    /// revoked token.
    #[allow(clippy::manual_find)]
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

    /// Grant (upsert) a role on a project for a token principal.
    pub fn grant(&self, project: &str, principal: &str, role: &str) -> Result<()> {
        if role != "read" && role != "write" {
            return Err(Error::invalid(format!(
                "role must be read or write, got '{role}'"
            )));
        }
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        conn.execute(
            "INSERT INTO project_grants (project, principal, role)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(project, principal) DO UPDATE SET role = excluded.role",
            params![project, principal, role],
        )
        .map_err(|e| Error::internal(format!("grant: {e}")))?;
        Ok(())
    }

    /// Remove a project grant for a principal.
    pub fn revoke_grant(&self, project: &str, principal: &str) -> Result<()> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        let n = conn
            .execute(
                "DELETE FROM project_grants WHERE project = ?1 AND principal = ?2",
                params![project, principal],
            )
            .map_err(|e| Error::internal(format!("revoke_grant: {e}")))?;
        if n == 0 {
            return Err(Error::not_found(format!(
                "grant for '{principal}' on '{project}'"
            )));
        }
        Ok(())
    }

    /// Role for a principal on a project, if any.
    pub fn grant_role(&self, project: &str, principal: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        conn.query_row(
            "SELECT role FROM project_grants WHERE project = ?1 AND principal = ?2",
            params![project, principal],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("grant_role: {e}")))
    }

    /// True when any grants are registered for the project (enables the gate).
    pub fn project_is_gated(&self, project: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM project_grants WHERE project = ?1)",
            [project],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v != 0)
        .map_err(|e| Error::internal(format!("project_is_gated: {e}")))
    }

    /// Full grant list (admin / reporting).
    pub fn list_grants(&self) -> Result<Vec<ProjectGrant>> {
        let conn = self.conn.lock().expect("tokens.db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT project, principal, role, granted_at FROM project_grants ORDER BY project, principal")
            .map_err(|e| Error::internal(format!("prepare grants: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ProjectGrant {
                    project: row.get(0)?,
                    principal: row.get(1)?,
                    role: row.get(2)?,
                    granted_at: row.get(3)?,
                })
            })
            .map_err(|e| Error::internal(format!("query grants: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| Error::internal(format!("grant rows: {e}")))?);
        }
        Ok(out)
    }
}

/// One project grant row, for admin display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGrant {
    pub project: String,
    pub principal: String,
    pub role: String,
    pub granted_at: String,
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

    #[test]
    fn grant_upsert_and_gate() {
        let (_d, s) = make_store();
        s.grant("proj-a", "alice", "write").unwrap();
        s.grant("proj-a", "bob", "read").unwrap();
        assert_eq!(
            s.grant_role("proj-a", "alice").unwrap().as_deref(),
            Some("write")
        );
        assert_eq!(
            s.grant_role("proj-a", "bob").unwrap().as_deref(),
            Some("read")
        );
        assert!(s.project_is_gated("proj-a").unwrap());
        assert!(!s.project_is_gated("proj-b").unwrap());

        // Upsert: alice downgraded to read.
        s.grant("proj-a", "alice", "read").unwrap();
        assert_eq!(
            s.grant_role("proj-a", "alice").unwrap().as_deref(),
            Some("read")
        );

        // Revoke.
        s.revoke_grant("proj-a", "bob").unwrap();
        assert_eq!(s.grant_role("proj-a", "bob").unwrap(), None);
        assert!(s.revoke_grant("proj-a", "ghost").is_err());

        let all = s.list_grants().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].principal, "alice");
    }

    #[test]
    fn grant_rejects_unknown_role() {
        let (_d, s) = make_store();
        assert!(s.grant("p", "x", "admin").is_err());
    }
}
