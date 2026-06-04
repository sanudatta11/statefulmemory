//! TS-11 — Session lifecycle (start → save obs → end → get) and the FR12.8
//! constraint that `session delete` returns FAILED_PRECONDITION when
//! observations still reference the session.

use memlayer_tests::CliEnv;
use serde_json::Value;

#[test]
fn ts11_session_lifecycle_and_delete_blocked() {
    let mut env = CliEnv::new();
    env.spawn_daemon();
    let session = uuid::Uuid::new_v4().to_string()[..8].to_string();

    // start
    let started: Value = json_cmd(&env, &["--output", "json", "session", "start", &session]);
    let s = &started["session"];
    assert_eq!(s["id"].as_str(), Some(session.as_str()));
    assert!(s["started_at"].as_str().is_some(), "started_at must be set; got: {started}");

    // attach an observation to the session
    let _: Value = json_cmd(&env, &[
        "--output", "json",
        "obs", "save",
        "--title", "before end",
        "--content", "first save",
        "--session", &session,
    ]);

    // end with summary
    let ended: Value = json_cmd(&env, &[
        "--output", "json",
        "session", "end", &session,
        "--summary", "wrap-up",
    ]);
    let s = &ended["session"];
    assert!(s["ended_at"].as_str().is_some(), "ended_at must be set after end");
    assert_eq!(s["summary"].as_str(), Some("wrap-up"));

    // get echoes both timestamps + summary
    let got: Value = json_cmd(&env, &["--output", "json", "session", "get", &session]);
    let s = &got["session"];
    assert!(s["started_at"].as_str().is_some());
    assert!(s["ended_at"].as_str().is_some());
    assert_eq!(s["summary"].as_str(), Some("wrap-up"));

    // delete must fail because the observation still references this session.
    let out = env
        .cmd()
        .args(["session", "delete", &session])
        .output()
        .expect("session delete");
    assert!(
        !out.status.success(),
        "delete should fail while obs references session; got {:?}",
        out.status,
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("FAILED_PRECONDITION")
            || stderr.to_lowercase().contains("references")
            || stderr.to_lowercase().contains("observation"),
        "stderr should mention the FK violation; got: {stderr}",
    );
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
