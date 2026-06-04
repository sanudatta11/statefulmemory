//! Project + token verbs.
//!
//! TS-12 — `project consolidate --dry-run` + `project prune` semantics.
//! TS-24 — EC-4 self-merge + EC-5 duplicate token name.

use memlayer_tests::{start_session, CliEnv};
use serde_json::Value;

#[test]
fn ts12_consolidate_dry_run_and_prune() {
    let mut env = CliEnv::new();
    env.spawn_daemon();

    // Build three projects.
    //
    //   alpha-svc → has 1 observation (must survive prune)
    //   alpha-srv → has 0 observations (prune candidate; also a consolidate
    //               candidate against alpha-svc — jaro_winkler of these two
    //               is well above 0.85)
    //   gamma     → has 0 observations (prune candidate; not a consolidate
    //               candidate of either)
    bootstrap_project(&env, "alpha-svc", true);
    bootstrap_project(&env, "alpha-srv", false);
    bootstrap_project(&env, "gamma", false);

    // SC-18: --dry-run must list candidates without merging.
    let consolidated: Value = json_cmd(&env, &[
        "--project", "alpha-svc",
        "--output", "json",
        "project", "consolidate", "--dry-run", "--all",
    ]);
    let candidates = consolidated["candidates"]
        .as_array()
        .expect("candidates array");
    assert!(
        candidates.iter().any(|c| {
            let from = c["from"].as_str().unwrap_or("");
            let to = c["to"].as_str().unwrap_or("");
            (from == "alpha-svc" && to == "alpha-srv")
                || (from == "alpha-srv" && to == "alpha-svc")
        }),
        "expected alpha-svc/alpha-srv pair in candidates; got: {consolidated}",
    );

    // SC-18: dry-run did not mutate — both projects still exist.
    let projects: Value = json_cmd(&env, &[
        "--project", "alpha-svc",
        "--output", "json",
        "project", "list",
    ]);
    let names: Vec<&str> = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["normalized_name"].as_str())
        .collect();
    assert!(names.contains(&"alpha-svc"), "alpha-svc should still exist");
    assert!(names.contains(&"alpha-srv"), "alpha-srv should still exist");

    // SC-19: `project prune --dry-run` lists 0-obs projects, with-obs survive.
    let pruned_dry: Value = json_cmd(&env, &[
        "--project", "alpha-svc",
        "--output", "json",
        "project", "prune", "--dry-run",
    ]);
    let would_remove: Vec<&str> = pruned_dry["would_remove"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert!(
        would_remove.contains(&"alpha-srv") && would_remove.contains(&"gamma"),
        "0-obs projects should be in would_remove; got: {pruned_dry}",
    );
    assert!(
        !would_remove.contains(&"alpha-svc"),
        "alpha-svc has obs and must not be in would_remove",
    );

    // SC-19: `project prune` actually removes the 0-obs projects, leaves alpha-svc.
    let pruned: Value = json_cmd(&env, &[
        "--project", "alpha-svc",
        "--output", "json",
        "project", "prune",
    ]);
    // would_remove returns the same list; the CLI then fans out DeleteProject.
    assert!(pruned["would_remove"].as_array().is_some());

    let after: Value = json_cmd(&env, &[
        "--project", "alpha-svc",
        "--output", "json",
        "project", "list",
    ]);
    let names_after: Vec<&str> = after["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["normalized_name"].as_str())
        .collect();
    assert!(names_after.contains(&"alpha-svc"), "alpha-svc must survive prune");
    assert!(!names_after.contains(&"gamma"), "gamma should have been removed");
    assert!(!names_after.contains(&"alpha-srv"), "alpha-srv should have been removed");
}

#[test]
fn ts24a_project_self_merge_invalid_argument() {
    // EC-4: `project merge --from X --to X` is INVALID_ARGUMENT → exit 2.
    let mut env = CliEnv::new();
    env.spawn_daemon();
    bootstrap_project(&env, "loop", true);

    let out = env
        .cmd()
        .args([
            "project", "merge",
            "--from", "loop",
            "--to", "loop",
        ])
        .output()
        .expect("project merge");
    assert_eq!(
        out.status.code(),
        Some(2),
        "self-merge should exit 2 (INVALID_ARGUMENT); stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn ts24b_duplicate_token_name_already_exists() {
    // EC-5: `team token-create --name existing` returns ALREADY_EXISTS → exit 1.
    // Run in UDS mode where the admin guard short-circuits, so we exercise
    // the dedup logic without standing up the TLS/TCP harness.
    let mut env = CliEnv::new();
    env.spawn_daemon();

    let _: Value = json_cmd(&env, &[
        "--output", "json",
        "team", "token-create",
        "--name", "alice",
    ]);

    let out = env
        .cmd()
        .args(["team", "token-create", "--name", "alice"])
        .output()
        .expect("token-create dup");
    assert!(
        !out.status.success(),
        "duplicate token-create should fail; got {:?}",
        out.status,
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "AlreadyExists maps to exit 1 (GENERAL) in non-lifecycle commands; stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.to_lowercase().contains("already") || stderr.contains("ALREADY_EXISTS"),
        "stderr should mention already-exists; got: {stderr}\n\
         daemon.stderr:\n{}",
        env.daemon_stderr().unwrap_or_else(|| "<none>".into()),
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn bootstrap_project(env: &CliEnv, name: &str, with_obs: bool) {
    // Each project is created lazily by the first session-start touching it.
    let session = format!("ses-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let out = env
        .cmd()
        .args(["--project", name, "session", "start", &session])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "session start --project {name} failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    if with_obs {
        let out = env
            .cmd()
            .args([
                "--project", name,
                "obs", "save",
                "--title", "anchor",
                "--content", "anchor body",
                "--session", &session,
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "obs save --project {name} failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }
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

#[allow(dead_code)]
fn _silence(_e: &CliEnv) {
    start_session(_e, "x");
}
