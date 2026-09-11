//! Cross-project --all-projects search routes through the global mirror DB
//! (aid-t6, aid-t7, aid-t8).
//!
//! Spec coverage: SC-9 — merged results across ≥2 projects, each tagged
//! with project_name; same topic_key across projects stays distinct.

use memlayer_tests::CliEnv;
use serde_json::Value;

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{n}")
}

fn save(env: &CliEnv, project: &str, session: &str, title: &str, content: &str, topic: Option<&str>) {
    // 1. Make sure the session exists in the target project.
    let mut start = env.cmd();
    start.env("MEMLAYER_PROJECT", project).args([
        "session", "start",
        session,
    ]);
    let out = start.output().expect("session start");
    assert!(
        out.status.success(),
        "session start in {project} failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    // 2. Save observation in that project.
    let mut cmd = env.cmd();
    cmd.env("MEMLAYER_PROJECT", project);
    let mut args = vec![
        "obs".to_string(), "save".to_string(),
        "--type".to_string(), "decision".to_string(),
        "--title".to_string(), title.to_string(),
        "--content".to_string(), content.to_string(),
        "--session".to_string(), session.to_string(),
    ];
    if let Some(t) = topic {
        args.push("--topic".into());
        args.push(t.into());
    }
    cmd.args(&args);
    let out = cmd.output().expect("obs save");
    assert!(
        out.status.success(),
        "obs save in {project} failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
}

// ---------------------------------------------------------------------------
// SC-9: --all-projects returns merged hits with project_name set.
// ---------------------------------------------------------------------------

#[test]
fn all_projects_returns_merged_results_with_project_tags() {
    let env = CliEnv::new();
    let session = uuid_short();

    save(&env, "repo-a", &session, "use pgx not gorm", "raw sql preferred", None);
    save(&env, "repo-b", &session, "kafka topic naming", "kebab-case", None);

    // Search from repo-a's perspective with --all-projects.
    let out = env
        .cmd()
        .env("MEMLAYER_PROJECT", "repo-a")
        .args([
            "obs", "search",
            "kafka OR pgx",
            "--all-projects",
            "--output", "json",
        ])
        .output()
        .expect("obs search --all-projects");
    assert!(
        out.status.success(),
        "search --all-projects failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).expect("json parse");
    let observations = parsed["observations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected observations array, got: {parsed}"));
    assert!(
        observations.len() >= 2,
        "expected ≥2 hits across projects, got {}: {parsed}",
        observations.len(),
    );

    let projects: Vec<String> = observations
        .iter()
        .filter_map(|o| o["project_name"].as_str().map(String::from))
        .collect();
    assert!(
        projects.iter().any(|p| p == "repo-a"),
        "missing repo-a hit, got projects: {projects:?}",
    );
    assert!(
        projects.iter().any(|p| p == "repo-b"),
        "missing repo-b hit, got projects: {projects:?}",
    );

    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// SC-9: same topic_key across two projects must NOT collapse.
// ---------------------------------------------------------------------------

#[test]
fn same_topic_key_across_projects_kept_distinct() {
    let env = CliEnv::new();
    let session = uuid_short();

    save(&env, "repo-a", &session, "auth via jwt", "validate in middleware", Some("auth"));
    save(&env, "repo-b", &session, "auth via session cookie", "redis-backed", Some("auth"));

    let out = env
        .cmd()
        .env("MEMLAYER_PROJECT", "repo-a")
        .args([
            "obs", "search",
            "auth",
            "--all-projects",
            "--output", "json",
        ])
        .output()
        .expect("obs search --all-projects");
    let parsed: Value = serde_json::from_slice(&out.stdout).expect("json parse");
    let observations = parsed["observations"].as_array().expect("array");

    // Both projects' auth observations must remain independent.
    let a_count = observations
        .iter()
        .filter(|o| o["project_name"].as_str() == Some("repo-a"))
        .count();
    let b_count = observations
        .iter()
        .filter(|o| o["project_name"].as_str() == Some("repo-b"))
        .count();
    assert!(a_count >= 1, "repo-a auth obs missing");
    assert!(b_count >= 1, "repo-b auth obs missing — same topic_key collapsed across projects");

    let _ = env.cmd().args(["daemon", "stop"]).output();
}
