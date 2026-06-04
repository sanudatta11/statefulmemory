//! TS-4, TS-5, TS-6, TS-16, TS-18, TS-27 — formatting, exit codes, and the
//! global behaviors of the `memlayer` CLI binary.

use std::process::Stdio;

use memlayer_tests::{start_session, CliEnv};
use serde_json::Value;

// ---------------------------------------------------------------------------
// TS-4 (SC-4, SC-5) — piped output → JSON; TTY-style → text table.
//
// `assert_cmd` cannot give the child a real PTY without pulling in extra
// platform-specific machinery, so we cover the piped→JSON branch (which is
// the actual zero-config behavior) and the TTY branch via the explicit
// `--output text` flag, which is what `Formatter::resolve` falls back to
// when stdout is a terminal.
// ---------------------------------------------------------------------------

#[test]
fn ts4_piped_default_is_json() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let out = env
        .cmd()
        .args(["daemon", "status"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("daemon status");
    assert!(out.status.success(), "daemon status failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let parsed: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("piped output was not JSON: {e}\noutput: {stdout}"));
    assert!(parsed.get("version").is_some(), "JSON missing `version` field: {stdout}");
}

#[test]
fn ts4_text_output_is_not_json() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let out = env
        .cmd()
        .args(["--output", "text", "daemon", "status"])
        .output()
        .expect("daemon status");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "text mode should not be parseable JSON; got: {stdout}",
    );
    assert!(stdout.contains("version"), "text mode should mention `version`: {stdout}");
}

// ---------------------------------------------------------------------------
// TS-5 (SC-6) — NO_COLOR=1 strips ANSI escapes.
//
// memlayer never emits ANSI in JSON mode and only colors text-mode output
// when running on a TTY. With piped stdout (no TTY) there is no ANSI to begin
// with — but NO_COLOR must also work in `--output text`, which is what we
// assert here.
// ---------------------------------------------------------------------------

#[test]
fn ts5_no_color_env_strips_ansi() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let out = env
        .cmd()
        .env("NO_COLOR", "1")
        .args(["--output", "text", "daemon", "status"])
        .output()
        .expect("daemon status");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains('\u{001b}'),
        "NO_COLOR=1 should strip ANSI escapes; got: {stdout:?}",
    );
}

// ---------------------------------------------------------------------------
// TS-6 (SC-7..SC-10) — four project-detection scenarios.
// ---------------------------------------------------------------------------

#[test]
fn ts6a_explicit_project_flag_wins() {
    // SC-7: --project flag overrides everything else.
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid_short();
    // Use cmd_no_project_env so MEMLAYER_PROJECT doesn't shadow the flag.
    let out = env
        .cmd_no_project_env()
        .args(["--project", "explicit-proj", "session", "start", &session])
        .output()
        .expect("session start");
    assert!(out.status.success(), "session start failed: {}", String::from_utf8_lossy(&out.stderr));

    // Confirm the session landed in `explicit-proj` by listing.
    let out = env
        .cmd_no_project_env()
        .args(["--project", "explicit-proj", "--output", "json", "session", "list"])
        .output()
        .expect("session list");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains(&session),
        "session id {session} should be in `explicit-proj`: {stdout}",
    );
}

#[test]
fn ts6b_memlayer_project_env_used() {
    // SC-8 (-ish): MEMLAYER_PROJECT env in absence of a --project flag.
    let mut env = CliEnv::new().with_project("env-proj");
    env.spawn_daemon();
    let session = uuid_short();
    let out = env
        .cmd()
        .args(["session", "start", &session])
        .output()
        .expect("session start");
    assert!(out.status.success());

    let out = env
        .cmd()
        .args(["--output", "json", "project", "current"])
        .output()
        .expect("project current");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("env-proj"),
        "MEMLAYER_PROJECT should set the current project; got: {stdout}",
    );
}

#[test]
fn ts6c_non_git_dir_returns_exit5() {
    // SC-10: in a non-git directory with no overrides, project detection
    // exits with code 5.
    let mut env = CliEnv::new();
    env.spawn_daemon();
    // No --project flag, no MEMLAYER_PROJECT env, working dir is the tempdir
    // (not a git repo).
    let out = env
        .cmd_no_project_env()
        .args(["session", "list"])
        .output()
        .expect("session list");
    assert_eq!(
        out.status.code(),
        Some(5),
        "non-git dir should exit 5; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.to_lowercase().contains("project") || stderr.to_lowercase().contains("git"),
        "stderr should hint at the detection failure: {stderr}",
    );
}

#[test]
fn ts6d_project_flag_overrides_env() {
    // Cross-check: --project beats MEMLAYER_PROJECT.
    let mut env = CliEnv::new().with_project("env-proj");
    env.spawn_daemon();
    let out = env
        .cmd()
        .args(["--project", "flag-proj", "--output", "json", "project", "current"])
        .output()
        .expect("project current");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("flag-proj"), "flag should win; got: {stdout}");
    assert!(!stdout.contains("env-proj"), "env should lose; got: {stdout}");
}

// ---------------------------------------------------------------------------
// TS-16 (SC-24) — version output matches the documented regex.
//
// Spec: `memlayer [0-9]+\.[0-9]+\.[0-9]+ \(commit [0-9a-f]+`
// We assert each part by hand to avoid pulling a regex crate.
// ---------------------------------------------------------------------------

#[test]
fn ts16_version_output_format() {
    let env = CliEnv::new();
    let out = env
        .cmd()
        .arg("version")
        .output()
        .expect("version");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let line = stdout.trim();

    let rest = line
        .strip_prefix("memlayer ")
        .unwrap_or_else(|| panic!("missing `memlayer ` prefix: {line:?}"));

    // X.Y.Z then space.
    let (semver, rest) = rest
        .split_once(' ')
        .unwrap_or_else(|| panic!("no space after semver: {line:?}"));
    let parts: Vec<&str> = semver.split('.').collect();
    assert_eq!(parts.len(), 3, "semver must have 3 parts: {semver}");
    for p in &parts {
        assert!(
            !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()),
            "semver part not digits: {p:?} in {line:?}",
        );
    }

    let after_paren = rest
        .strip_prefix("(commit ")
        .unwrap_or_else(|| panic!("missing `(commit ` segment: {line:?}"));
    let sha: String = after_paren
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    assert!(
        !sha.is_empty(),
        "expected hex commit sha after `(commit `: {line:?}",
    );
}

// ---------------------------------------------------------------------------
// TS-18 (SC-26) — unrecognized flag → exit 2 with usage on stderr.
// ---------------------------------------------------------------------------

#[test]
fn ts18_unknown_flag_exits_2() {
    let env = CliEnv::new();
    let out = env
        .cmd()
        .args(["obs", "recent", "--definitely-not-a-flag"])
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(2),
        "unknown flag should exit 2; stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.to_lowercase().contains("usage")
            || stderr.to_lowercase().contains("error")
            || stderr.contains("--help"),
        "stderr should give usage hint: {stderr}",
    );
}

// ---------------------------------------------------------------------------
// TS-27 (EH-1) — error messages on stderr; stdout empty on error.
// ---------------------------------------------------------------------------

#[test]
fn ts27_errors_go_to_stderr_only() {
    // No daemon spawned → `obs recent` should fail (auto-spawn either succeeds
    // or times out; we drive an explicit error path instead by giving an
    // invalid id to `obs get`).
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let out = env
        .cmd()
        .args(["obs", "get", "nonexistent-sync-id"])
        .output()
        .expect("obs get");
    assert!(!out.status.success(), "obs get on missing id should fail");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stdout.trim().is_empty(),
        "EH-1: stdout must be empty on error; got: {stdout:?}",
    );
    assert!(
        !stderr.trim().is_empty(),
        "EH-1: error message must appear on stderr",
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn uuid_short() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

// Silence "unused" warnings from helpers used only by future tests.
#[allow(dead_code)]
fn _silence_unused(_e: &CliEnv) {
    start_session(_e, "x");
}
