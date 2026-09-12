//! Code anchors: parse/serialize, content digests, and DB helpers.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
use sha2::{Digest, Sha256};

use memlayer_core::error::{Error, Result};

/// Binding of an observation to a repository artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub symbol: Option<String>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub anchor_commit: Option<String>,
    pub content_digest: Option<String>,
}

/// Freshness state, independent of confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyState {
    Unanchored,
    Verified,
    Stale,
    Invalidated,
    Unprovable,
}

impl VerifyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unanchored => "unanchored",
            Self::Verified => "verified",
            Self::Stale => "stale",
            Self::Invalidated => "invalidated",
            Self::Unprovable => "unprovable",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "verified" => Self::Verified,
            "stale" => Self::Stale,
            "invalidated" => Self::Invalidated,
            "unprovable" => Self::Unprovable,
            _ => Self::Unanchored,
        }
    }

    /// Worst of two states (`Verified < Stale < Invalidated < Unprovable`).
    pub fn worse(self, other: Self) -> Self {
        use VerifyState::*;
        let rank = |s: Self| match s {
            Unanchored => 0,
            Verified => 1,
            Stale => 2,
            Invalidated => 3,
            Unprovable => 4,
        };
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}

impl Anchor {
    /// Accept `path`, `path::symbol`, `path::symbol::line`, `path:12-40`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::invalid("empty code anchor"));
        }
        // path:12-40 form (single colon, numeric range)
        if let Some((path, rest)) = s.split_once(':') {
            if !path.contains("::") && rest.chars().any(|c| c.is_ascii_digit()) {
                if let Some((a, b)) = rest.split_once('-') {
                    if let (Ok(start), Ok(end)) = (a.parse::<u32>(), b.parse::<u32>()) {
                        return Ok(Self {
                            path: path.replace('\\', "/"),
                            symbol: None,
                            line_start: Some(start),
                            line_end: Some(end),
                            anchor_commit: None,
                            content_digest: None,
                        });
                    }
                }
            }
        }
        let parts: Vec<&str> = s.split("::").collect();
        let path = parts[0].replace('\\', "/");
        let symbol = parts.get(1).map(|p| (*p).to_string()).filter(|p| !p.is_empty());
        let line = parts.get(2).and_then(|p| p.parse::<u32>().ok());
        Ok(Self {
            path,
            symbol,
            line_start: line,
            line_end: line,
            anchor_commit: None,
            content_digest: None,
        })
    }

    pub fn to_canonical_string(&self) -> String {
        match (&self.symbol, self.line_start, self.line_end) {
            (Some(sym), Some(a), Some(b)) if a == b => format!("{}::{sym}::{a}", self.path),
            (Some(sym), Some(a), Some(b)) => format!("{}::{sym}:{a}-{b}", self.path),
            (Some(sym), _, _) => format!("{}::{sym}", self.path),
            (None, Some(a), Some(b)) if a != b => format!("{}:{a}-{b}", self.path),
            (None, Some(a), _) => format!("{}::{a}", self.path),
            _ => self.path.clone(),
        }
    }
}

impl std::fmt::Display for Anchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_canonical_string())
    }
}

/// Normalize line endings before hashing so CRLF churn is not a change.
fn normalize_eol(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Sha256 hex of the anchored slice (line range, symbol window, or whole file).
pub fn digest_slice(file_text: &str, anchor: &Anchor) -> String {
    let text = normalize_eol(file_text);
    let lines: Vec<&str> = text.lines().collect();
    let slice = if let (Some(start), Some(end)) = (anchor.line_start, anchor.line_end) {
        let s = start.saturating_sub(1) as usize;
        let e = (end as usize).min(lines.len()).max(s);
        lines[s..e].join("\n")
    } else if let Some(sym) = anchor.symbol.as_deref() {
        if let Some((start, end)) = locate_symbol(&text, sym) {
            let s = start.saturating_sub(1) as usize;
            let e = (end as usize).min(lines.len()).max(s);
            lines[s..e].join("\n")
        } else {
            text.clone()
        }
    } else {
        text.clone()
    };
    let mut h = Sha256::new();
    h.update(slice.as_bytes());
    hex::encode(h.finalize())
}

/// First line containing `symbol` as a whole word, extended to the end of its
/// indentation block. No AST parsing.
pub fn locate_symbol(file_text: &str, symbol: &str) -> Option<(u32, u32)> {
    let text = normalize_eol(file_text);
    let lines: Vec<&str> = text.lines().collect();
    let mut start_idx = None;
    for (i, line) in lines.iter().enumerate() {
        if contains_whole_word(line, symbol) {
            start_idx = Some(i);
            break;
        }
    }
    let start = start_idx?;
    let base_indent = leading_indent(lines[start]);
    let mut end = start;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            end = i;
            continue;
        }
        let ind = leading_indent(line);
        if ind <= base_indent && !line.trim_start().starts_with('}') && !line.trim_start().starts_with(')') {
            // Same or lower indent non-closer → end of block at previous line.
            break;
        }
        end = i;
    }
    Some((start as u32 + 1, end as u32 + 1))
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn contains_whole_word(line: &str, word: &str) -> bool {
    let bytes = line.as_bytes();
    let w = word.as_bytes();
    if w.is_empty() {
        return false;
    }
    let mut i = 0;
    while i + w.len() <= bytes.len() {
        if &bytes[i..i + w.len()] == w {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + w.len() == bytes.len() || !is_ident_byte(bytes[i + w.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub fn insert_anchors(conn: &Connection, observation_id: i64, anchors: &[Anchor]) -> Result<()> {
    for a in anchors {
        conn.execute(
            "INSERT OR REPLACE INTO observation_anchors
                (observation_id, path, symbol, line_start, line_end, anchor_commit, content_digest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                observation_id,
                a.path,
                a.symbol,
                a.line_start.map(|n| n as i64),
                a.line_end.map(|n| n as i64),
                a.anchor_commit,
                a.content_digest,
            ],
        )
        .map_err(|e| Error::internal(format!("insert_anchors: {e}")))?;
    }
    Ok(())
}

pub fn anchors_for(conn: &Connection, observation_id: i64) -> Result<Vec<Anchor>> {
    let mut stmt = conn
        .prepare(
            "SELECT path, symbol, line_start, line_end, anchor_commit, content_digest
             FROM observation_anchors WHERE observation_id = ?1 ORDER BY id ASC",
        )
        .map_err(|e| Error::internal(format!("anchors_for prepare: {e}")))?;
    let rows = stmt
        .query_map(params![observation_id], |row| {
            Ok(Anchor {
                path: row.get(0)?,
                symbol: row.get(1)?,
                line_start: row.get::<_, Option<i64>>(2)?.map(|n| n as u32),
                line_end: row.get::<_, Option<i64>>(3)?.map(|n| n as u32),
                anchor_commit: row.get(4)?,
                content_digest: row.get(5)?,
            })
        })
        .map_err(|e| Error::internal(format!("anchors_for query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("anchors_for row: {e}")))?);
    }
    Ok(out)
}

pub fn set_verify_state(
    conn: &Connection,
    observation_id: i64,
    state: VerifyState,
    verified_commit: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE observations
            SET verify_state = ?2,
                verified_commit = ?3,
                verified_at = datetime('now')
          WHERE id = ?1",
        params![observation_id, state.as_str(), verified_commit],
    )
    .map_err(|e| Error::internal(format!("set_verify_state: {e}")))?;
    Ok(())
}

/// List observations that have at least one anchor row.
pub fn list_anchored(conn: &Connection, limit: i64) -> Result<Vec<(i64, Vec<Anchor>)>> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT observation_id FROM observation_anchors
             ORDER BY observation_id ASC LIMIT ?1",
        )
        .map_err(|e| Error::internal(format!("list_anchored prepare: {e}")))?;
    let ids: Vec<i64> = stmt
        .query_map(params![limit], |row| row.get(0))
        .map_err(|e| Error::internal(format!("list_anchored query: {e}")))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::internal(format!("list_anchored row: {e}")))?;
    let mut out = Vec::new();
    for id in ids {
        out.push((id, anchors_for(conn, id)?));
    }
    Ok(out)
}

/// Fetch verify_state for an observation (defaults to unanchored).
pub fn verify_state_of(conn: &Connection, observation_id: i64) -> Result<VerifyState> {
    let s: Option<String> = conn
        .query_row(
            "SELECT verify_state FROM observations WHERE id = ?1",
            params![observation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| Error::internal(format!("verify_state_of: {e}")))?;
    Ok(s.map(|v| VerifyState::parse(&v)).unwrap_or(VerifyState::Unanchored))
}

/// Batch-load anchors for many observation ids.
pub fn anchors_for_many(
    conn: &Connection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<Anchor>>> {
    use std::collections::HashMap;
    let mut out: HashMap<i64, Vec<Anchor>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT observation_id, path, symbol, line_start, line_end, anchor_commit, content_digest
         FROM observation_anchors WHERE observation_id IN ({placeholders})
         ORDER BY observation_id ASC, id ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("anchors_for_many prepare: {e}")))?;
    let params: Vec<&dyn ToSql> = ids.iter().map(|id| id as &dyn ToSql).collect();
    let rows = stmt
        .query_map(params_from_iter(params), |row| {
            let oid: i64 = row.get(0)?;
            let a = Anchor {
                path: row.get(1)?,
                symbol: row.get(2)?,
                line_start: row.get::<_, Option<i64>>(3)?.map(|n| n as u32),
                line_end: row.get::<_, Option<i64>>(4)?.map(|n| n as u32),
                anchor_commit: row.get(5)?,
                content_digest: row.get(6)?,
            };
            Ok((oid, a))
        })
        .map_err(|e| Error::internal(format!("anchors_for_many query: {e}")))?;
    for r in rows {
        let (oid, a) = r.map_err(|e| Error::internal(format!("anchors_for_many row: {e}")))?;
        out.entry(oid).or_default().push(a);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips() {
        let cases = [
            "src/auth.rs",
            "src/auth.rs::validate",
            "src/auth.rs::validate::12",
            "src/auth.rs:12-40",
        ];
        for c in cases {
            let a = Anchor::parse(c).unwrap();
            let back = Anchor::parse(&a.to_canonical_string()).unwrap();
            assert_eq!(a.path, back.path, "path for {c}");
        }
    }

    #[test]
    fn digest_stable_across_crlf() {
        let a = Anchor::parse("f.rs:1-2").unwrap();
        let d1 = digest_slice("hello\nworld\n", &a);
        let d2 = digest_slice("hello\r\nworld\r\n", &a);
        assert_eq!(d1, d2);
    }

    #[test]
    fn locate_symbol_rust_and_python() {
        let rust = "fn helper() {}\n\npub fn validate_token(s: &str) {\n    let x = 1;\n    x\n}\n\nfn other() {}\n";
        let (s, e) = locate_symbol(rust, "validate_token").unwrap();
        assert_eq!(s, 3);
        assert!(e >= s);
        let py = "def helper():\n    pass\n\ndef validate_token(s):\n    x = 1\n    return x\n\ndef other():\n    pass\n";
        let (s, e) = locate_symbol(py, "validate_token").unwrap();
        assert_eq!(s, 4);
        assert!(e >= s);
    }

    #[test]
    fn verify_state_worse_order() {
        assert_eq!(
            VerifyState::Verified.worse(VerifyState::Stale),
            VerifyState::Stale
        );
        assert_eq!(
            VerifyState::Stale.worse(VerifyState::Unprovable),
            VerifyState::Unprovable
        );
    }
}
