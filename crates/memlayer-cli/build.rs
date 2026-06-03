//! Build script for `memlayer-cli`. Emits `MEMLAYER_GIT_SHA` and
//! `MEMLAYER_BUILD_TS` as compile-time env vars so the `version` subcommand
//! can produce `memlayer X.Y.Z (commit <sha>, built <date>)` per FR11 / SC-24.
//!
//! Plain std + `git` shell-out — no `vergen` dep — to keep the build graph
//! lean and to avoid a workspace-dep churn for a 30-line task. The Unix
//! timestamp is formatted at runtime via the regular `chrono` dep so the
//! build script itself stays dependency-free.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let sha = git_short_sha();
    println!("cargo:rustc-env=MEMLAYER_GIT_SHA={sha}");

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=MEMLAYER_BUILD_TS={ts}");

    // Re-run when HEAD or any local branch ref moves so the SHA stays fresh.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
}

fn git_short_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
