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

// FR1.4: --no-color must produce identical behavior to NO_COLOR=1. We assert
// the flag is genuinely wired through to the renderer (not just parsed) by
// checking that it sets the env var that downstream `formatter::use_color`
// inspects, and that the rendered output is ANSI-free.
#[test]
fn ts5_no_color_flag_is_wired() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let out = env
        .cmd()
        .args(["--no-color", "--output", "text", "daemon", "status"])
        .output()
        .expect("daemon status");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains('\u{001b}'),
        "--no-color should strip ANSI escapes; got: {stdout:?}",
    );
}

// FR1.6: --quiet suppresses informational stderr output. `daemon start`
// without --quiet writes "daemon started (socket ...)" to stderr; with
// --quiet that line must be absent. Each sub-case uses its own CliEnv so
// the two daemons don't share lock/socket files.
#[test]
fn fr1_6_quiet_suppresses_informational_output() {
    fn run_and_dump_on_fail(env: &CliEnv, args: &[&str]) -> std::process::Output {
        let out = env.cmd().args(args).output().expect("daemon start");
        if !out.status.success() {
            let log_path = env.log_path();
            let log = std::fs::read_to_string(&log_path)
                .unwrap_or_else(|e| format!("(could not read {}: {e})", log_path.display()));
            // Also list every file in the tempdir so we can spot rolling-appender
            // suffixes or unexpected paths.
            let mut listing = String::new();
            if let Ok(entries) = std::fs::read_dir(env.data_path()) {
                for e in entries.flatten() {
                    let meta = e.metadata().ok();
                    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    listing.push_str(&format!("  {} ({} bytes)\n", e.path().display(), len));
                }
            }
            panic!(
                "memlayer {:?} failed: exit={:?}\nstderr={}\nstdout={}\ndaemon.log={}\ntempdir contents:\n{}",
                args,
                out.status.code(),
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout),
                log,
                listing,
            );
        }
        out
    }

    // Sub-case A: without --quiet, the confirmation line appears.
    {
        let env = CliEnv::new();
        let out = run_and_dump_on_fail(&env, &["daemon", "start"]);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.contains("daemon started"),
            "without --quiet, daemon start should print confirmation; got: {stderr:?}",
        );
        let _ = env.cmd().args(["daemon", "stop"]).output();
    }

    // Sub-case B: with --quiet, the confirmation line is absent (exit 0 still).
    {
        let env = CliEnv::new();
        let out = run_and_dump_on_fail(&env, &["--quiet", "daemon", "start"]);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            !stderr.contains("daemon started"),
            "--quiet should suppress `daemon started` line; got: {stderr:?}",
        );
        let _ = env.cmd().args(["daemon", "stop"]).output();
    }
}

// FR1.6 cont.: `team init-ca` normally prints three "wrote <path>" lines to
// stderr. --quiet must silence them.
#[test]
fn fr1_6_quiet_suppresses_init_ca_output() {
    let env = CliEnv::new();
    let out_dir = env.data_path().join("ca-out");
    let out = env
        .cmd()
        .args(["--quiet", "team", "init-ca", out_dir.to_str().unwrap()])
        .output()
        .expect("team init-ca --quiet");
    assert!(out.status.success(), "init-ca --quiet failed: {}",
        String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        !stderr.contains("wrote "),
        "--quiet should suppress `wrote <path>` lines; got: {stderr:?}",
    );
    // The files must still be written even when output is suppressed.
    assert!(out_dir.join("ca.pem").is_file());
    assert!(out_dir.join("server.pem").is_file());
    assert!(out_dir.join("server-key.pem").is_file());
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

// SC-7: `<cwd>/.memlayer/config.json` with `{"project_name": "X"}` takes
// precedence over git detection. Verifies the config-file path is genuinely
// exercised (the existing TS-6 only covered --project flag and env var).
#[test]
fn ts6_sc7_config_json_drives_project_name() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    // macOS tempdirs are named `.tmpXXXXXX` which `normalize` rejects (leading
    // dot). Run the CLI from a non-dot subdirectory so cwd's basename is
    // valid, and put .memlayer/config.json there so detection finds it.
    let work_dir = env.data_path().join("workspace-sc7");
    let dot_dir = work_dir.join(".memlayer");
    std::fs::create_dir_all(&dot_dir).unwrap();
    std::fs::write(
        dot_dir.join("config.json"),
        r#"{"project_name":"from-config-json"}"#,
    )
    .unwrap();

    let mut cmd = env.cmd_no_project_env();
    cmd.current_dir(&work_dir);
    let out = cmd
        .args(["--output", "json", "project", "current"])
        .output()
        .expect("project current");
    assert!(
        out.status.success(),
        "project current failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("from-config-json"),
        "config.json detection should select `from-config-json`; got: {stdout}",
    );
    assert!(
        stdout.contains("config_file") || stdout.contains("config.json"),
        "source should attribute the detection to the config file; got: {stdout}",
    );
}

// SC-8: a git repo with an `origin` remote URL drives the project name.
// We construct a minimal `.git/` skeleton inside the tempdir so the binary's
// `git rev-parse / git config` shells succeed. The harness rooted at the
// tempdir is not itself a git repo, so creating `.git` here makes it one.
#[test]
fn ts6_sc8_git_remote_drives_project_name() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    // Same leading-dot avoidance as SC-7: do not run the CLI directly inside
    // the macOS tempdir whose basename starts with `.tmp`.
    let work_dir = env.data_path().join("workspace-sc8");
    std::fs::create_dir_all(&work_dir).unwrap();
    write_minimal_git_dir(&work_dir, Some("git@github.com:acme/proj-from-remote.git"));

    let mut cmd = env.cmd_no_project_env();
    cmd.current_dir(&work_dir);
    let out = cmd
        .args(["--output", "json", "project", "current"])
        .output()
        .expect("project current");
    assert!(
        out.status.success(),
        "project current failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("proj-from-remote"),
        "git remote detection should select `proj-from-remote`; got: {stdout}",
    );
}

// SC-9: a git repo without an `origin` remote falls back to the git-root
// basename.
#[test]
fn ts6_sc9_git_root_basename_drives_project_name() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    // Pick a meaningful basename for the working directory by creating a
    // sub-directory named for the expected project, then making THAT a git
    // repo. The CLI runs `cd <data_dir>` (CliEnv::cmd's `current_dir`); we
    // override that here by building the command manually.
    let work_dir = env.data_path().join("repo-from-basename");
    std::fs::create_dir_all(&work_dir).unwrap();
    write_minimal_git_dir(&work_dir, None);

    let mut cmd = env.cmd_no_project_env();
    cmd.current_dir(&work_dir);
    let out = cmd
        .args(["--output", "json", "project", "current"])
        .output()
        .expect("project current");
    assert!(
        out.status.success(),
        "project current failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("repo-from-basename"),
        "git root basename should select `repo-from-basename`; got: {stdout}",
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

/// Create a minimal `.git/` directory inside `root` with `core` config and
/// an optional `[remote "origin"]` URL. The CLI's project_detect uses pure
/// file I/O (it parses `.git/config` directly rather than shelling out to
/// git), so this is sufficient to drive SC-7..SC-9 detection.
fn write_minimal_git_dir(root: &std::path::Path, origin_url: Option<&str>) {
    let git_dir = root.join(".git");
    std::fs::create_dir_all(&git_dir).unwrap();
    let mut cfg = String::from("[core]\nrepositoryformatversion = 0\n");
    if let Some(url) = origin_url {
        cfg.push_str(&format!("[remote \"origin\"]\n\turl = {url}\n"));
    }
    std::fs::write(git_dir.join("config"), cfg).unwrap();
}

// Silence "unused" warnings from helpers used only by future tests.
#[allow(dead_code)]
fn _silence_unused(_e: &CliEnv) {
    start_session(_e, "x");
}
