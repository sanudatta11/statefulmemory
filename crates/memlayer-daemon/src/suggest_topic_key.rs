//! `SuggestTopicKey` heuristic (FR12.5, OQ-1).
//!
//! Given a candidate observation `(type, title, scope)`, propose a stable
//! `topic_key` of the form `<family>/<slug>` that does not collide with an
//! existing non-deleted observation in the same scope.
//!
//! Algorithm:
//! 1. Map `type` to a fixed family prefix (`decision`, `policy`, …).
//! 2. Kebab-slug the title (lowercase, alnum + `-`, collapse runs, max 40
//!    chars).
//! 3. Compose `family/slug`. If this exact key already exists in the scope,
//!    append `-2`, `-3`, … until unique.
//!
//! The hard taxonomy here is intentional (PRD OQ-1: "hard taxonomy …"). The
//! heuristic is deterministic so the same input always produces the same
//! suggestion, modulo collision counters.

use rusqlite::{params, Connection};

use memlayer_core::error::{Error, Result};

/// Fixed map from observation `type` to topic-key family prefix.
///
/// Unknown types fall back to `general`. The mapping is intentionally
/// lossless (preserves the type name) so power users can grep topic keys by
/// family across the corpus.
pub fn family_prefix(obs_type: &str) -> &'static str {
    match obs_type {
        "decision" => "decision",
        "policy" => "policy",
        "preference" => "preference",
        "note" => "note",
        "learning" => "learning",
        "config" => "config",
        _ => "general",
    }
}

/// Convert `title` to a kebab-case slug suitable for a topic-key suffix.
///
/// - Lowercased.
/// - Each maximal run of non-alphanumeric characters becomes a single `-`.
/// - Leading/trailing `-` trimmed.
/// - Truncated to 40 chars (then re-trimmed to avoid trailing `-`).
/// - Empty input becomes `untitled`.
pub fn kebab_slug(title: &str) -> String {
    const MAX: usize = 40;
    let mut out = String::with_capacity(title.len());
    let mut last_dash = false;
    for c in title.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            out.push(lc);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.chars().count() > MAX {
        let truncated: String = out.chars().take(MAX).collect();
        out = truncated;
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("untitled");
    }
    out
}

/// Produce a non-colliding topic key for `(obs_type, title, scope)`.
///
/// `conn` is a read connection on the project DB. The collision check looks
/// at non-soft-deleted observations only; reusing the topic_key of a
/// soft-deleted row is intentional (the soft-delete is a tombstone, not a
/// reservation).
pub fn suggest(
    conn: &Connection,
    obs_type: &str,
    title: &str,
    scope: &str,
) -> Result<String> {
    if title.trim().is_empty() {
        return Err(Error::invalid("title is required for suggest-topic-key"));
    }
    let family = family_prefix(obs_type);
    let slug = kebab_slug(title);
    let base = format!("{family}/{slug}");

    // Fast path: no collision.
    if !key_exists(conn, &base, scope)? {
        return Ok(base);
    }

    // Collision: append -2, -3, … until unique. Cap at 9999 to avoid runaway.
    for n in 2..=9999 {
        let candidate = format!("{base}-{n}");
        if !key_exists(conn, &candidate, scope)? {
            return Ok(candidate);
        }
    }
    Err(Error::FailedPrecondition(format!(
        "could not find non-colliding topic key for '{base}' in scope '{scope}'"
    )))
}

fn key_exists(conn: &Connection, key: &str, scope: &str) -> Result<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM observations
              WHERE topic_key = ?1 AND scope = ?2 AND deleted_at IS NULL",
            params![key, scope],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("collision check: {e}")))?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memlayer_storage::db;
    use tempfile::TempDir;

    fn open_db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let conn = db::open_write(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, directory) VALUES ('s1', '/tmp')",
            [],
        )
        .unwrap();
        (dir, conn)
    }

    fn insert_with_topic(conn: &Connection, topic_key: &str, scope: &str) {
        conn.execute(
            "INSERT INTO observations
                (sync_id, session_id, type, title, content, scope, topic_key)
              VALUES (?1, 's1', 'note', 't', 'c', ?2, ?3)",
            params![uuid::Uuid::new_v4().to_string(), scope, topic_key],
        )
        .unwrap();
    }

    #[test]
    fn kebab_slug_and_family_prefix() {
        // Slug basics.
        assert_eq!(kebab_slug("Auth Strategy: JWT"), "auth-strategy-jwt");
        assert_eq!(kebab_slug("  Hello   World  "), "hello-world");
        assert_eq!(kebab_slug("special!@#chars$$$"), "special-chars");
        assert_eq!(kebab_slug(""), "untitled");
        // Truncation cap is 40 chars and trims any trailing dash.
        let long_input = "a".repeat(60);
        let s = kebab_slug(&long_input);
        assert!(s.len() <= 40);
        // Family prefix.
        assert_eq!(family_prefix("decision"), "decision");
        assert_eq!(family_prefix("policy"), "policy");
        assert_eq!(family_prefix("preference"), "preference");
        assert_eq!(family_prefix("unknown"), "general");
    }

    #[test]
    fn collision_detection_returns_unique() {
        let (_d, conn) = open_db();
        // Reserve the base key in the project scope.
        insert_with_topic(&conn, "decision/auth-strategy", "project");

        // Ask for the same title — should append -2.
        let s = suggest(&conn, "decision", "Auth Strategy", "project").unwrap();
        assert_eq!(s, "decision/auth-strategy-2");

        // Reserve -2 too; next suggestion should be -3.
        insert_with_topic(&conn, "decision/auth-strategy-2", "project");
        let s2 = suggest(&conn, "decision", "Auth Strategy", "project").unwrap();
        assert_eq!(s2, "decision/auth-strategy-3");
    }

    #[test]
    fn collision_check_is_scoped() {
        let (_d, conn) = open_db();
        // Reserve in personal scope only.
        insert_with_topic(&conn, "decision/auth-strategy", "personal");

        // project scope is still free.
        let s = suggest(&conn, "decision", "Auth Strategy", "project").unwrap();
        assert_eq!(s, "decision/auth-strategy");
    }

    #[test]
    fn empty_title_is_rejected() {
        let (_d, conn) = open_db();
        let r = suggest(&conn, "note", "   ", "project");
        assert!(matches!(r, Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn soft_deleted_topic_can_be_reused() {
        let (_d, conn) = open_db();
        insert_with_topic(&conn, "note/foo", "project");
        // Soft-delete the existing row.
        conn.execute(
            "UPDATE observations SET deleted_at = datetime('now') WHERE topic_key = 'note/foo'",
            [],
        )
        .unwrap();
        // Suggestion should reuse the base, not collide with the tombstone.
        let s = suggest(&conn, "note", "Foo", "project").unwrap();
        assert_eq!(s, "note/foo");
    }
}
