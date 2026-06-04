//! `obs` subcommand integration tests.
//!
//! TS-1, TS-7, TS-8, TS-9, TS-10, TS-19, TS-20, TS-21, TS-22, TS-23, TS-25.

use std::io::Write;
use std::process::Stdio;

use memlayer_tests::{start_session, CliEnv};
use serde_json::Value;

// ---------------------------------------------------------------------------
// TS-1 (SC-1) — cold-machine `obs save` auto-spawns the daemon.
//
// We do not pre-spawn the daemon. The CLI's open_client() must detect the
// missing socket, take the auto-spawn flock, fork+detach a daemon, poll until
// the socket appears, and complete the save. Verifies FR2 + SC-1 end-to-end.
// ---------------------------------------------------------------------------

#[test]
fn ts1_cold_machine_auto_spawn() {
    let env = CliEnv::new();
    let session = uuid_short();
    // Sessions need to exist before saves; the very first call (`session
    // start`) is also responsible for triggering auto-spawn.
    start_session(&env, &session);
    assert!(
        env.socket_path().exists(),
        "socket should exist after auto-spawn at {}",
        env.socket_path().display(),
    );

    // Now save an observation against the auto-spawned daemon.
    let out = env
        .cmd()
        .args([
            "obs", "save",
            "--title", "auto",
            "--content", "hello cold-start",
            "--session", &session,
        ])
        .output()
        .expect("obs save");
    assert!(
        out.status.success(),
        "obs save failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    // Tell the auto-spawned daemon to shut down so the OS reaps it before
    // the tempdir is removed.
    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// TS-7 (SC-11) — save → update --title → get; revision_count = 2.
// ---------------------------------------------------------------------------

#[test]
fn ts7_save_update_get_bumps_revision() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let saved: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save",
        "--title", "first",
        "--content", "body v1",
        "--session", &session,
    ]);
    let id = saved["observation"]["sync_id"]
        .as_str()
        .expect("sync_id")
        .to_string();
    assert_eq!(saved["observation"]["revision_count"].as_i64(), Some(1));

    let _updated: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "update", &id,
        "--title", "second",
    ]);

    let got: Value = json_cmd(&env, &["--output", "json", "obs", "get", &id]);
    assert_eq!(got["observation"]["title"], "second");
    assert_eq!(
        got["observation"]["revision_count"].as_i64(),
        Some(2),
        "revision_count must bump after update; got: {got}",
    );
}

// ---------------------------------------------------------------------------
// TS-8 (SC-12, SC-13) — soft delete hides; hard delete removes.
// ---------------------------------------------------------------------------

#[test]
fn ts8_soft_delete_hides_hard_delete_removes() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let soft: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save", "--title", "soft", "--content", "soft body",
        "--session", &session,
    ]);
    let soft_id = soft["observation"]["sync_id"].as_str().unwrap().to_string();

    let hard: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save", "--title", "hard", "--content", "hard body",
        "--session", &session,
    ]);
    let hard_id = hard["observation"]["sync_id"].as_str().unwrap().to_string();

    // Soft delete.
    let out = env.cmd().args(["obs", "delete", &soft_id]).output().unwrap();
    assert!(out.status.success(), "soft delete failed: {}", String::from_utf8_lossy(&out.stderr));

    // SC-12: soft-deleted obs hidden from get.
    let out = env.cmd().args(["obs", "get", &soft_id]).output().unwrap();
    assert!(!out.status.success(), "soft-deleted obs should not be retrievable: {:?}", out);

    // SC-13: hard delete with --hard.
    let out = env
        .cmd()
        .args(["obs", "delete", &hard_id, "--hard"])
        .output()
        .unwrap();
    assert!(out.status.success(), "hard delete failed: {}", String::from_utf8_lossy(&out.stderr));

    let out = env.cmd().args(["obs", "get", &hard_id]).output().unwrap();
    assert!(!out.status.success(), "hard-deleted obs should not be retrievable");
}

// ---------------------------------------------------------------------------
// TS-9 (SC-14) — `obs context` returns recent observations.
// ---------------------------------------------------------------------------

#[test]
fn ts9_context_returns_recent_three() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);
    for i in 0..3 {
        let title = format!("ctx-{i}");
        let out = env
            .cmd()
            .args([
                "obs", "save",
                "--title", &title,
                "--content", &format!("content for {i}"),
                "--session", &session,
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "save {i} failed");
    }

    let v: Value = json_cmd(&env, &["--output", "json", "obs", "context", "--limit", "10"]);
    let recents = v["recent_observations"].as_array().expect("recent_observations array");
    assert!(
        recents.len() >= 3,
        "expected >= 3 recent observations; got {}",
        recents.len(),
    );
}

// ---------------------------------------------------------------------------
// TS-10 (SC-15) — capture-passive with two bullets → two saved obs.
// ---------------------------------------------------------------------------

#[test]
fn ts10_capture_passive_two_bullets() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let markdown = "Some prologue.\n\n## Key Learnings:\n- prefer pgx over rusqlite\n- always use prepared statements\n\nDone.\n";

    let out = env
        .cmd()
        .args([
            "--output", "json",
            "obs", "capture-passive",
            "--text", markdown,
            "--session", &session,
        ])
        .output()
        .expect("capture-passive");
    assert!(
        out.status.success(),
        "capture-passive failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let saved = v["saved"].as_array().expect("saved array");
    assert_eq!(saved.len(), 2, "expected 2 saved obs; got: {v}");
}

// ---------------------------------------------------------------------------
// TS-19 (SC-27) — `obs list --due-for-review` filters by review_after.
//
// A fresh decision has review_after 6 months in the future, so it is not due.
// To exercise the "is due" branch we open the project DB directly and rewind
// review_after to 2020. This is the cleanest way to test the filter without
// waiting six months or leaking time-mocking infrastructure into the daemon.
// ---------------------------------------------------------------------------

#[test]
fn ts19_due_for_review_filter() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    // Save a decision (gets review_after = now + 6 months).
    let saved: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save",
        "--type", "decision",
        "--title", "use postgres",
        "--content", "decided to use postgres for the new service",
        "--session", &session,
    ]);
    let sync_id = saved["observation"]["sync_id"].as_str().unwrap().to_string();

    // Fresh: not due.
    let v: Value = json_cmd(&env, &["--output", "json", "obs", "list", "--due-for-review"]);
    let empty: Vec<Value> = Vec::new();
    let observations = v["observations"].as_array().unwrap_or(&empty);
    assert!(
        observations.iter().all(|o| o["sync_id"].as_str() != Some(&sync_id)),
        "fresh decision should not be due-for-review; got: {v}",
    );

    // Rewind review_after directly in the project DB.
    let db = project_db_path(&env);
    let conn = rusqlite::Connection::open(&db).expect("open project db");
    let n = conn
        .execute(
            "UPDATE observations SET review_after = '2020-01-01T00:00:00Z' WHERE sync_id = ?1",
            rusqlite::params![sync_id],
        )
        .expect("rewind review_after");
    assert_eq!(n, 1, "expected to update exactly one row");
    drop(conn);

    // Now it must appear in --due-for-review.
    let v: Value = json_cmd(&env, &["--output", "json", "obs", "list", "--due-for-review"]);
    let empty: Vec<Value> = Vec::new();
    let observations = v["observations"].as_array().unwrap_or(&empty);
    assert!(
        observations.iter().any(|o| o["sync_id"].as_str() == Some(&sync_id)),
        "rewound decision should be due-for-review; got: {v}",
    );
}

// ---------------------------------------------------------------------------
// TS-20 (SC-28) — suggest-topic-key returns a non-colliding `<family>/<slug>`.
// ---------------------------------------------------------------------------

#[test]
fn ts20_suggest_topic_key_non_collision() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let v: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "suggest-topic-key",
        "--title", "use Postgres for storage",
        "--type", "decision",
    ]);
    let key = v["topic_key"].as_str().expect("topic_key").to_string();
    assert!(
        key.contains('/'),
        "expected `<family>/<slug>` format; got: {key:?}",
    );
    let parts: Vec<&str> = key.splitn(2, '/').collect();
    assert_eq!(parts.len(), 2);
    let slug = parts[1];
    assert!(
        slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "slug must be kebab-case ASCII; got: {slug:?}",
    );

    // Save a decision with this exact topic; a second suggest should produce a
    // different (non-colliding) key.
    let _: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save",
        "--type", "decision",
        "--title", "use Postgres for storage",
        "--content", "...",
        "--topic", &key,
        "--session", &session,
    ]);

    let v2: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "suggest-topic-key",
        "--title", "use Postgres for storage",
        "--type", "decision",
    ]);
    let key2 = v2["topic_key"].as_str().expect("topic_key");
    assert_ne!(key2, key, "second suggestion must avoid collision; got: {key2}");
}

// ---------------------------------------------------------------------------
// TS-21 (SC-29) — 12 rows + `obs list --limit 5` + cursor → 3 pages.
// ---------------------------------------------------------------------------

#[test]
fn ts21_pagination_three_pages() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    for i in 0..12 {
        let title = format!("p{i:02}");
        let out = env
            .cmd()
            .args([
                "obs", "save",
                "--title", &title,
                "--content", &format!("body {i}"),
                "--session", &session,
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "save {i} failed");
    }

    let mut total = 0usize;
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut argv: Vec<String> = vec![
            "--output".into(),
            "json".into(),
            "obs".into(),
            "list".into(),
            "--limit".into(),
            "5".into(),
        ];
        if let Some(c) = &cursor {
            argv.push("--cursor".into());
            argv.push(c.clone());
        }
        let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let v: Value = json_cmd(&env, &argv_refs);
        let observations = v["observations"].as_array().expect("observations array");
        total += observations.len();
        pages += 1;
        let next = v["next_cursor"].as_str().unwrap_or("");
        if next.is_empty() {
            break;
        }
        cursor = Some(next.to_string());
        if pages > 6 {
            panic!("pagination did not terminate within 6 pages; got: {v}");
        }
    }
    assert_eq!(pages, 3, "expected exactly 3 pages for 12 rows at limit 5");
    assert_eq!(total, 12, "expected 12 total observations across pages");
}

// ---------------------------------------------------------------------------
// TS-22 (EC-2) — FTS5-special characters do not panic the search verb.
// ---------------------------------------------------------------------------

#[test]
fn ts22_fts5_special_chars_no_panic() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let _: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save",
        "--title", "literal hyphen", "--content", "uniqueneedlexyz body",
        "--session", &session,
    ]);

    // Each of these queries is a torture test for an FTS5 tokenizer; the
    // daemon must sanitize and never crash. We don't care about hits — only
    // that the verb returns 0 cleanly.
    let queries = [
        "needle-with-hyphens",
        "\"unterminated quote",
        "AND OR NOT",
        ":colon-prefix",
        "()(()",
        "NEAR/3 boom",
    ];
    for q in queries {
        let out = env
            .cmd()
            .args(["--output", "json", "obs", "search", q])
            .output()
            .expect("obs search");
        assert!(
            out.status.success(),
            "obs search {q:?} should not crash; stderr={}",
            String::from_utf8_lossy(&out.stderr),
        );
        let _: Value = serde_json::from_slice(&out.stdout).expect("valid json");
    }
}

// ---------------------------------------------------------------------------
// TS-23 (EC-3) — `capture-passive` without a `## Key Learnings:` header
//                returns an empty list and exits 0.
// ---------------------------------------------------------------------------

#[test]
fn ts23_capture_passive_no_header_empty_list() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let out = env
        .cmd()
        .args([
            "--output", "json",
            "obs", "capture-passive",
            "--text", "Just some prose with no headers.\n",
            "--session", &session,
        ])
        .output()
        .expect("capture-passive");
    assert!(out.status.success(), "exit 0 expected; got {:?}", out.status);
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let saved = v["saved"].as_array().expect("saved array");
    assert!(saved.is_empty(), "expected 0 saved obs; got: {v}");
}

// ---------------------------------------------------------------------------
// TS-25 (EC-7) — oversized stdin to `obs save --content -` → exit 2.
// ---------------------------------------------------------------------------

#[test]
fn ts25_oversized_stdin_exits_2() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    start_session(&env, &session);

    let oversized = "x".repeat(50_001);
    let mut child = env
        .cmd()
        .args([
            "obs", "save",
            "--title", "big",
            "--content", "-",
            "--session", &session,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn obs save");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        // Best-effort write; the child may close stdin once it hits the cap.
        let _ = stdin.write_all(oversized.as_bytes());
    }
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(2),
        "oversized stdin should exit 2 (USAGE / INVALID_ARGUMENT); stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn uuid_short() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

fn json_cmd(env: &CliEnv, args: &[&str]) -> Value {
    let out = env.cmd().args(args).output().expect("cmd");
    if !out.status.success() {
        panic!(
            "command {:?} failed (exit={:?}): stderr={}",
            args,
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "command {:?} stdout was not valid JSON: {e}\nstdout: {}",
            args,
            String::from_utf8_lossy(&out.stdout),
        )
    })
}

fn project_db_path(env: &CliEnv) -> std::path::PathBuf {
    env.data_path()
        .join("projects")
        .join(format!("{}.db", env.project))
}
