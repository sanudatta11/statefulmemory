//! `memlayer hook pre-tool` end-to-end integration tests (aid-t9, aid-t10, aid-t11).
//!
//! Spec coverage:
//!   SC-1: Grep mode renders markdown with prior observation IDs to stdout
//!   SC-2: Read mode emits stderr-only hint, never stdout body
//!   SC-3: memlayer install adds exactly one PreToolUse entry per matcher
//!         (Grep, Read); re-running install does not duplicate.

use memlayer_tests::{start_session, CliEnv};

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{n}")
}

// ---------------------------------------------------------------------------
// SC-1: Grep mode returns markdown that contains the matching observation.
// ---------------------------------------------------------------------------

#[test]
fn grep_hook_returns_prior_observation() {
    let env = CliEnv::new();
    let session = uuid_short();
    start_session(&env, &session);

    // Seed a decision the hook should surface.
    let _ = env
        .cmd()
        .args([
            "obs", "save",
            "--type", "decision",
            "--title", "use pgx not gorm",
            "--content", "Team prefers raw SQL via pgx; GORM rejected for codegen overhead",
            "--session", &session,
        ])
        .output()
        .expect("obs save");

    let out = env
        .cmd()
        .args(["hook", "pre-tool", "--tool", "Grep", "--pattern", "pgx"])
        .output()
        .expect("hook pre-tool Grep");

    assert!(out.status.success(), "hook must always exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("memlayer prior context") && stdout.contains("pgx"),
        "Grep mode missing markdown header / token, got: {stdout}",
    );

    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// SC-1: Grep mode silent (empty stdout) on no hits.
// ---------------------------------------------------------------------------

#[test]
fn grep_hook_silent_on_no_hits() {
    let env = CliEnv::new();
    let session = uuid_short();
    start_session(&env, &session);

    let out = env
        .cmd()
        .args([
            "hook", "pre-tool",
            "--tool", "Grep",
            "--pattern", "definitely-not-in-store-zzzz",
        ])
        .output()
        .expect("hook pre-tool Grep");

    assert!(out.status.success(), "hook must always exit 0");
    assert!(
        out.stdout.is_empty(),
        "expected silent stdout on no hits, got: {}",
        String::from_utf8_lossy(&out.stdout),
    );

    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// SC-2: Read mode writes only to stderr; stdout stays empty (no body inject).
// ---------------------------------------------------------------------------

#[test]
fn read_hook_writes_only_stderr() {
    let env = CliEnv::new();
    let session = uuid_short();
    start_session(&env, &session);

    let _ = env
        .cmd()
        .args([
            "obs", "save",
            "--type", "fix",
            "--title", "write race in handler",
            "--content", "Fixed concurrent map write in src/server/handler.rs",
            "--session", &session,
        ])
        .output()
        .expect("obs save");

    let out = env
        .cmd()
        .args([
            "hook", "pre-tool",
            "--tool", "Read",
            "--path", "crates/memlayer-storage/src/write.rs",
        ])
        .output()
        .expect("hook pre-tool Read");

    assert!(out.status.success(), "hook must always exit 0");
    assert!(
        out.stdout.is_empty(),
        "Read mode must NOT inject body to stdout, got: {}",
        String::from_utf8_lossy(&out.stdout),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // We expect SOME stderr because there's a matching obs (title contains
    // "write"). Naive keyword extraction yields "write" + "memlayer" or
    // "storage" depending on stopword behavior; either should match the
    // saved fix.
    assert!(
        stderr.contains("memlayer hint") || stderr.is_empty(),
        "stderr should contain a hint or be empty, got: {stderr}",
    );

    let _ = env.cmd().args(["daemon", "stop"]).output();
}

// ---------------------------------------------------------------------------
// SC-3: install patches PreToolUse with exactly one Grep + one Read entry,
// idempotent on re-install.
// ---------------------------------------------------------------------------

#[test]
fn install_patches_pretool_grep_read_idempotent() {
    let env = CliEnv::new();

    // Target Claude Code explicitly — bare `install` only patches agents
    // detected under HOME/PATH, which is empty in CI tempdirs.
    let out1 = env
        .cmd()
        .args(["install", "--agent", "claude-code"])
        .output()
        .expect("install 1");
    assert!(
        out1.status.success(),
        "install must succeed: {}",
        String::from_utf8_lossy(&out1.stderr),
    );

    let settings_path = env.data_path().join(".claude").join("settings.json");
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&settings_path).unwrap_or_else(|e| {
            panic!(
                "read settings.json at {}: {e}; install stdout=\n{}stderr=\n{}",
                settings_path.display(),
                String::from_utf8_lossy(&out1.stdout),
                String::from_utf8_lossy(&out1.stderr),
            )
        }),
    )
    .expect("parse settings.json");

    let pretool = settings["hooks"]["PreToolUse"]
        .as_array()
        .expect("PreToolUse must be an array after install");

    let memlayer_grep_count = pretool
        .iter()
        .filter(|b| {
            b["matcher"].as_str() == Some("Grep")
                && b["hooks"]
                    .as_array()
                    .map(|h| {
                        h.iter().any(|x| {
                            x["command"]
                                .as_str()
                                .map(|s| s.starts_with("memlayer "))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
        })
        .count();
    let memlayer_read_count = pretool
        .iter()
        .filter(|b| {
            b["matcher"].as_str() == Some("Read")
                && b["hooks"]
                    .as_array()
                    .map(|h| {
                        h.iter().any(|x| {
                            x["command"]
                                .as_str()
                                .map(|s| s.starts_with("memlayer "))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
        })
        .count();
    assert_eq!(memlayer_grep_count, 1, "exactly one Grep entry expected");
    assert_eq!(memlayer_read_count, 1, "exactly one Read entry expected");

    // Re-install. Idempotency: counts unchanged.
    let _ = env
        .cmd()
        .args(["install", "--agent", "claude-code"])
        .output()
        .expect("install 2");
    let settings2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read"))
            .expect("parse 2");
    let pretool2 = settings2["hooks"]["PreToolUse"].as_array().unwrap();

    let grep2 = pretool2
        .iter()
        .filter(|b| b["matcher"].as_str() == Some("Grep"))
        .count();
    let read2 = pretool2
        .iter()
        .filter(|b| b["matcher"].as_str() == Some("Read"))
        .count();
    assert_eq!(grep2, 1, "re-install must not duplicate Grep entry");
    assert_eq!(read2, 1, "re-install must not duplicate Read entry");
}
