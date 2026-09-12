//! Anchor verification engine — pure relative to git helpers.

use std::collections::HashMap;
use std::path::Path;

use memlayer_core::git;
use memlayer_storage::anchor::{digest_slice, locate_symbol, Anchor, VerifyState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorVerdict {
    pub observation_id: i64,
    pub state: VerifyState,
    pub reason: &'static str,
}

/// Verify anchors against `repo` at `head`. Untouched paths re-verify for free
/// (one `git diff` per distinct `anchor_commit`, cached for the call).
pub fn verify_anchors(
    repo: &Path,
    head: &str,
    anchors: &[(i64, Anchor)],
) -> Vec<AnchorVerdict> {
    let mut changed_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut by_obs: HashMap<i64, (VerifyState, &'static str)> = HashMap::new();

    for (obs_id, anchor) in anchors {
        let (state, reason) = verify_one(repo, head, anchor, &mut changed_cache);
        by_obs
            .entry(*obs_id)
            .and_modify(|(s, r)| {
                let next = s.worse(state);
                if next != *s {
                    *s = next;
                    *r = reason;
                }
            })
            .or_insert((state, reason));
    }

    let mut out: Vec<AnchorVerdict> = by_obs
        .into_iter()
        .map(|(observation_id, (state, reason))| AnchorVerdict {
            observation_id,
            state,
            reason,
        })
        .collect();
    out.sort_by_key(|v| v.observation_id);
    out
}

fn verify_one(
    repo: &Path,
    head: &str,
    anchor: &Anchor,
    changed_cache: &mut HashMap<String, Vec<String>>,
) -> (VerifyState, &'static str) {
    let Some(commit) = anchor.anchor_commit.as_deref() else {
        return (VerifyState::Unanchored, "no anchor_commit");
    };
    if commit == head {
        return (VerifyState::Verified, "at head");
    }

    let changed = changed_cache.entry(commit.to_string()).or_insert_with(|| {
        git::changed_paths(repo, commit, head).unwrap_or_default()
    });
    let path_norm = anchor.path.replace('\\', "/");
    if !changed.iter().any(|p| p == &path_norm) {
        return (VerifyState::Verified, "path unchanged");
    }

    let file = match git::file_at_commit(repo, head, &path_norm) {
        Ok(Some(t)) => t,
        Ok(None) => return (VerifyState::Unprovable, "file absent at head"),
        Err(_) => return (VerifyState::Unprovable, "git show failed"),
    };

    if let Some(sym) = anchor.symbol.as_deref() {
        if locate_symbol(&file, sym).is_none() {
            return (VerifyState::Invalidated, "symbol missing");
        }
    }

    if let Some(expected) = anchor.content_digest.as_deref() {
        let got = digest_slice(&file, anchor);
        if got == expected {
            return (VerifyState::Verified, "digest unchanged");
        }
        return (VerifyState::Stale, "digest changed");
    }

    // Path changed and no digest to compare — treat as stale.
    (VerifyState::Stale, "path changed, no digest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        let _ = Command::new("git")
            .args(["config", "user.email", "t@example.com"])
            .current_dir(dir.path())
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(dir.path())
            .status();
        dir
    }

    fn commit_all(dir: &Path, msg: &str) -> String {
        let _ = Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .status();
        assert!(Command::new("git")
            .args(["commit", "-m", msg])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        git::head_sha(dir).unwrap()
    }

    #[test]
    fn untouched_path_stays_verified() {
        let dir = init_repo();
        fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        let c1 = commit_all(dir.path(), "c1");
        fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();
        let c2 = commit_all(dir.path(), "c2");
        let anchor = Anchor {
            path: "a.rs".into(),
            symbol: None,
            line_start: None,
            line_end: None,
            anchor_commit: Some(c1.clone()),
            content_digest: Some(digest_slice("fn a() {}\n", &Anchor::parse("a.rs").unwrap())),
        };
        let v = verify_anchors(dir.path(), &c2, &[(1, anchor)]);
        assert_eq!(v[0].state, VerifyState::Verified);
    }

    #[test]
    fn changed_digest_is_stale() {
        let dir = init_repo();
        fs::write(dir.path().join("a.rs"), "fn a() { 1 }\n").unwrap();
        let c1 = commit_all(dir.path(), "c1");
        let a0 = Anchor::parse("a.rs:1-1").unwrap();
        let digest = digest_slice("fn a() { 1 }\n", &a0);
        fs::write(dir.path().join("a.rs"), "fn a() { 2 }\n").unwrap();
        let c2 = commit_all(dir.path(), "c2");
        let anchor = Anchor {
            path: "a.rs".into(),
            symbol: None,
            line_start: Some(1),
            line_end: Some(1),
            anchor_commit: Some(c1),
            content_digest: Some(digest),
        };
        let v = verify_anchors(dir.path(), &c2, &[(7, anchor)]);
        assert_eq!(v[0].state, VerifyState::Stale);
    }

    #[test]
    fn deleted_file_is_unprovable() {
        let dir = init_repo();
        fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        let c1 = commit_all(dir.path(), "c1");
        assert!(Command::new("git")
            .args(["rm", "a.rs"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        let c2 = commit_all(dir.path(), "rm");
        let anchor = Anchor {
            path: "a.rs".into(),
            symbol: None,
            line_start: None,
            line_end: None,
            anchor_commit: Some(c1),
            content_digest: Some("dead".into()),
        };
        let v = verify_anchors(dir.path(), &c2, &[(1, anchor)]);
        assert_eq!(v[0].state, VerifyState::Unprovable);
    }
}
