//! Integration tests for the JSONL audit log (aid-t1, aid-t2, aid-t3).
//!
//! Spec coverage: SC-6 (metadata-only by default), SC-7 (full mode adds
//! query + top_hits), SC-10 (fail-silent on read-only log path).

use std::io::BufRead;

use memlayer_tests::{start_session, CliEnv};

fn read_log_lines(env: &CliEnv) -> Vec<serde_json::Value> {
    let log_path = env.data_path().join("queries.log");
    let f = match std::fs::File::open(&log_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    std::io::BufReader::new(f)
        .lines()
        .filter_map(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{n}")
}

// ---------------------------------------------------------------------------
// SC-6: metadata mode logs ts, command, project, result_count, duration_ms
// ---------------------------------------------------------------------------

#[test]
fn metadata_mode_logs_required_keys() {
    let env = CliEnv::new();
    let session = uuid_short();
    start_session(&env, &session);

    let _ = env
        .cmd()
        .args([
            "obs", "save",
            "--title", "audit-test-1",
            "--content", "checking metadata mode",
            "--session", &session,
        ])
        .output()
        .expect("obs save");

    let _ = env
        .cmd()
        .args(["obs", "search", "audit-test-1"])
        .output()
        .expect("obs search");

    let lines = read_log_lines(&env);
    assert!(
        lines.len() >= 2,
        "expected ≥2 audit lines, got {} in {:?}",
        lines.len(),
        lines,
    );

    for line in &lines {
        // Required keys per SC-6:
        assert!(line.get("ts").is_some(), "missing ts: {line}");
        assert!(line.get("command").is_some(), "missing command: {line}");
        assert!(line.get("duration_ms").is_some(), "missing duration_ms: {line}");

        // In metadata mode, query and top_hits MUST be absent.
        assert!(
            line.get("query").is_none(),
            "query must be absent in metadata mode: {line}",
        );
        assert!(
            line.get("top_hits").is_none(),
            "top_hits must be absent in metadata mode: {line}",
        );
    }

    // At least one line should be obs.save and one obs.search.
    let cmds: Vec<_> = lines
        .iter()
        .filter_map(|l| l.get("command").and_then(|c| c.as_str()))
        .collect();
    assert!(cmds.contains(&"obs.save"), "missing obs.save in {cmds:?}");
    assert!(cmds.contains(&"obs.search"), "missing obs.search in {cmds:?}");

    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// SC-7: MEMLAYER_AUDIT_FULL=1 adds query + top_hits
// ---------------------------------------------------------------------------

#[test]
fn full_mode_adds_query_and_top_hits() {
    let env = CliEnv::new();
    let session = uuid_short();
    start_session(&env, &session);

    let _ = env
        .cmd()
        .args([
            "obs", "save",
            "--title", "full-mode-target",
            "--content", "should appear in top_hits",
            "--session", &session,
        ])
        .output()
        .expect("obs save");

    let _ = env
        .cmd()
        .env("MEMLAYER_AUDIT_FULL", "1")
        .args(["obs", "search", "full-mode-target"])
        .output()
        .expect("obs search full");

    let lines = read_log_lines(&env);
    let search_line = lines
        .iter()
        .find(|l| l.get("command").and_then(|c| c.as_str()) == Some("obs.search"))
        .expect("search line missing");

    assert_eq!(
        search_line.get("query").and_then(|v| v.as_str()),
        Some("full-mode-target"),
        "full mode must record query: {search_line}",
    );
    let hits = search_line
        .get("top_hits")
        .and_then(|v| v.as_array())
        .expect("top_hits missing in full mode");
    assert!(!hits.is_empty(), "top_hits empty");
    let hit = &hits[0];
    assert!(hit.get("id").is_some(), "hit missing id: {hit}");
    assert!(hit.get("type").is_some(), "hit missing type: {hit}");
    assert!(hit.get("title").is_some(), "hit missing title: {hit}");

    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// SC-10: a read-only / unwritable log path does not break obs save.
// ---------------------------------------------------------------------------

#[test]
fn unwritable_log_does_not_break_save() {
    let env = CliEnv::new();
    let session = uuid_short();
    start_session(&env, &session);

    // Pre-create queries.log as a directory so any attempt to open it as a
    // file with O_APPEND fails. The audit recorder must swallow the error.
    let log_path = env.data_path().join("queries.log");
    std::fs::create_dir_all(&log_path).expect("mkdir as log path");

    let out = env
        .cmd()
        .args([
            "obs", "save",
            "--title", "save-despite-bad-log",
            "--content", "the save itself must succeed",
            "--session", &session,
        ])
        .output()
        .expect("obs save");

    assert!(
        out.status.success(),
        "obs save must succeed even when queries.log is unwritable; stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let _ = env.cmd().args(["daemon", "stop"]).output();
}
