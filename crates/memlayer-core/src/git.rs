//! Thin wrappers around the `git` binary (no git2/gix dependency).
//!
//! Every helper returns `Result` and tolerates "not a repo". Callers must never
//! let a git failure propagate as a save failure.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::error::{Error, Result};

const GIT_TIMEOUT: Duration = Duration::from_secs(10);

fn strip_proxy(cmd: &mut Command) {
    cmd.env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("no_proxy")
        .env_remove("NO_PROXY");
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let dir = dir.to_path_buf();
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let args_label = args.join(" ");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut cmd = Command::new("git");
        strip_proxy(&mut cmd);
        cmd.args(&args)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let _ = tx.send(cmd.output());
    });
    match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Ok(out)) => {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(Error::internal(format!("git {args_label} failed: {err}")))
            }
        }
        Ok(Err(e)) => Err(Error::internal(format!("git spawn: {e}"))),
        Err(_) => Err(Error::internal("git timed out")),
    }
}

/// True when `dir` is inside a git working tree.
pub fn is_repo(dir: &Path) -> bool {
    run_git(dir, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s == "true")
        .unwrap_or(false)
}

/// Current HEAD commit SHA (full 40-hex when available).
pub fn head_sha(dir: &Path) -> Result<String> {
    let sha = run_git(dir, &["rev-parse", "HEAD"])?;
    if sha.is_empty() {
        return Err(Error::internal("empty HEAD sha"));
    }
    Ok(sha)
}

/// Repo-relative paths changed between `from` and `to` (exclusive..inclusive style
/// of `git diff --name-only from..to`).
pub fn changed_paths(dir: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let range = format!("{from}..{to}");
    let out = run_git(dir, &["diff", "--name-only", &range])?;
    Ok(out
        .lines()
        .map(|l| l.trim().replace('\\', "/"))
        .filter(|l| !l.is_empty())
        .collect())
}

/// File contents at `commit:path`. `None` when the path does not exist there.
pub fn file_at_commit(dir: &Path, commit: &str, path: &str) -> Result<Option<String>> {
    let spec = format!("{commit}:{path}");
    match run_git(dir, &["show", &spec]) {
        Ok(s) => Ok(Some(s)),
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("does not exist")
                || msg.contains("exists on disk")
                || msg.contains("path") && msg.contains("exist")
                || msg.contains("fatal: path")
                || msg.contains("bad object")
            {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }
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
            .output()
            .unwrap()
            .status
            .success());
        // Identity for commit.
        let _ = Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir.path())
            .status();
        dir
    }

    #[test]
    fn is_repo_false_for_plain_dir() {
        let dir = TempDir::new().unwrap();
        assert!(!is_repo(dir.path()));
    }

    #[test]
    fn head_and_changed_paths_round_trip() {
        let dir = init_repo();
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(is_repo(dir.path()));
        let sha1 = head_sha(dir.path()).unwrap();
        assert!(!sha1.is_empty());

        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["commit", "-m", "update"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        let sha2 = head_sha(dir.path()).unwrap();
        let changed = changed_paths(dir.path(), &sha1, &sha2).unwrap();
        assert!(changed.iter().any(|p| p == "a.txt"));
        let body = file_at_commit(dir.path(), &sha2, "a.txt").unwrap();
        assert_eq!(body.as_deref(), Some("two"));
    }
}
