//! Fail-silent JSONL audit log for memlayer CLI calls.
//!
//! Every `obs` / `session` / `hook` verb appends one line to
//! `~/.memlayer/queries.log` capturing timestamp, command, project, result
//! count, and duration. With `MEMLAYER_AUDIT_FULL=1` the line additionally
//! carries the query string and the `{id, type, title}` of returned
//! observations (truncated to 4 KB total).
//!
//! The recorder is fail-silent by contract: a read-only log path, a full
//! disk, or a serialization error MUST NOT propagate to the caller. The
//! audit log is for debugging retrieval — it can never break a save.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

/// One JSONL row in `~/.memlayer/queries.log`.
#[derive(Debug, Serialize)]
pub struct AuditEntry<'a> {
    pub ts: String,
    pub command: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
    pub duration_ms: u128,
    /// Only populated when `MEMLAYER_AUDIT_FULL=1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Only populated when `MEMLAYER_AUDIT_FULL=1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_hits: Option<Vec<HitMeta>>,
}

/// Compact identifier for a returned observation (full-mode only).
#[derive(Debug, Serialize)]
pub struct HitMeta {
    pub id: i64,
    pub r#type: String,
    pub title: String,
}

/// Maximum bytes for any single string field in full mode.
const MAX_FIELD_BYTES: usize = 4096;

/// `~/.memlayer/queries.log` (or `$MEMLAYER_DATA_DIR/queries.log`).
fn log_path() -> PathBuf {
    memlayer_core::paths::data_dir().join("queries.log")
}

/// True when `MEMLAYER_AUDIT_FULL` is set to a non-empty, non-`0` value.
pub fn full_mode_enabled() -> bool {
    std::env::var_os("MEMLAYER_AUDIT_FULL")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// Append one JSONL line. Silently drops the entry on any I/O or
/// serialization failure — never propagates to the caller.
pub fn record(entry: &AuditEntry<'_>) {
    let mut entry = clone_truncated(entry);

    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let Ok(mut line) = serde_json::to_vec(&entry) else {
        return;
    };
    line.push(b'\n');
    let _ = file.write_all(&line);
    // Drop the entry; flush is implicit on drop, errors swallowed.
    let _ = entry.query.take();
}

/// Convenience for `Utc::now()` formatted as RFC 3339.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn clone_truncated<'a>(entry: &AuditEntry<'a>) -> AuditEntry<'a> {
    AuditEntry {
        ts: entry.ts.clone(),
        command: entry.command,
        project: entry.project,
        result_count: entry.result_count,
        duration_ms: entry.duration_ms,
        query: entry.query.as_ref().map(|s| truncate_chars(s, MAX_FIELD_BYTES)),
        top_hits: entry.top_hits.as_ref().map(|hits| {
            hits.iter()
                .map(|h| HitMeta {
                    id: h.id,
                    r#type: truncate_chars(&h.r#type, 32),
                    title: truncate_chars(&h.title, MAX_FIELD_BYTES),
                })
                .collect()
        }),
    }
}

fn truncate_chars(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Find the last char boundary at or below max_bytes - 13 (room for "…[truncated]").
    let suffix = "…[truncated]";
    let limit = max_bytes.saturating_sub(suffix.len());
    let mut end = limit;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + suffix.len());
    out.push_str(&s[..end]);
    out.push_str(suffix);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    fn with_tmp_data_dir<F: FnOnce(&std::path::Path)>(f: F) {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("MEMLAYER_DATA_DIR");
        std::env::set_var("MEMLAYER_DATA_DIR", tmp.path());
        f(tmp.path());
        match prev {
            Some(v) => std::env::set_var("MEMLAYER_DATA_DIR", v),
            None => std::env::remove_var("MEMLAYER_DATA_DIR"),
        }
    }

    #[test]
    fn record_appends_jsonl_line() {
        with_tmp_data_dir(|dir| {
            let entry = AuditEntry {
                ts: now_rfc3339(),
                command: "obs.search",
                project: Some("test-proj"),
                result_count: Some(3),
                duration_ms: 12,
                query: None,
                top_hits: None,
            };
            record(&entry);

            let log = std::fs::File::open(dir.join("queries.log")).unwrap();
            let lines: Vec<String> = std::io::BufReader::new(log)
                .lines()
                .map(|l| l.unwrap())
                .collect();
            assert_eq!(lines.len(), 1);
            let parsed: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
            assert_eq!(parsed["command"], "obs.search");
            assert_eq!(parsed["project"], "test-proj");
            assert_eq!(parsed["result_count"], 3);
        });
    }

    #[test]
    fn full_mode_env_var_toggles() {
        std::env::remove_var("MEMLAYER_AUDIT_FULL");
        assert!(!full_mode_enabled());
        std::env::set_var("MEMLAYER_AUDIT_FULL", "1");
        assert!(full_mode_enabled());
        std::env::set_var("MEMLAYER_AUDIT_FULL", "0");
        assert!(!full_mode_enabled());
        std::env::set_var("MEMLAYER_AUDIT_FULL", "");
        assert!(!full_mode_enabled());
        std::env::remove_var("MEMLAYER_AUDIT_FULL");
    }

    #[test]
    fn record_silent_on_unwritable_path() {
        // Point data_dir at a path that can't be created (parent is a file).
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let blocked = tmp.path().join("queries.log"); // tmp.path() is a regular file, .join().parent()==tmp.path() => create_dir_all fails
        std::env::set_var("MEMLAYER_DATA_DIR", &blocked);

        let entry = AuditEntry {
            ts: now_rfc3339(),
            command: "obs.search",
            project: None,
            result_count: None,
            duration_ms: 0,
            query: None,
            top_hits: None,
        };
        // Must not panic.
        record(&entry);
        std::env::remove_var("MEMLAYER_DATA_DIR");
    }

    #[test]
    fn content_truncated_at_4kb() {
        let big = "a".repeat(10_000);
        let out = truncate_chars(&big, MAX_FIELD_BYTES);
        assert!(out.len() <= MAX_FIELD_BYTES);
        assert!(out.ends_with("…[truncated]"));
    }
}
