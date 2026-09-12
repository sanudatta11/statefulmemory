//! Git hooks that re-verify code anchors after commit / merge / checkout.
//!
//! Marker-delimited blocks so re-install is idempotent and uninstall leaves
//! any surrounding user hook content intact.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const HOOK_START: &str = "# >>> memlayer >>>";
pub const HOOK_END: &str = "# <<< memlayer <<<";

const HOOK_BODY: &str = r#"command -v memlayer >/dev/null 2>&1 && \
  (memlayer verify --quiet >/dev/null 2>&1 &)
exit 0"#;

const HOOK_NAMES: &[&str] = &["post-commit", "post-merge", "post-checkout"];

/// Resolve `.git/hooks` for `dir` (supports worktrees via `git rev-parse`).
pub fn hooks_dir(dir: &Path) -> Option<PathBuf> {
    if !crate_is_repo(dir) {
        return None;
    }
    let out = Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "--git-path", "hooks"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if rel.is_empty() {
        return None;
    }
    let p = PathBuf::from(&rel);
    if p.is_absolute() {
        Some(p)
    } else {
        Some(dir.join(p))
    }
}

fn crate_is_repo(dir: &Path) -> bool {
    memlayer_core::git::is_repo(dir)
}

/// Install memlayer blocks into post-commit / post-merge / post-checkout.
/// Returns the number of hook files created or updated.
pub fn install_git_hooks(repo: &Path) -> std::io::Result<usize> {
    let Some(dir) = hooks_dir(repo) else {
        return Ok(0);
    };
    fs::create_dir_all(&dir)?;
    let mut changed = 0;
    for name in HOOK_NAMES {
        let path = dir.join(name);
        if install_hook_file(&path)? {
            changed += 1;
        }
        // Always ensure executable bit (even if content unchanged).
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }
    Ok(changed)
}

/// Remove memlayer marker blocks from the three hooks. Leaves other content.
pub fn remove_git_hooks(repo: &Path) -> std::io::Result<usize> {
    let Some(dir) = hooks_dir(repo) else {
        return Ok(0);
    };
    let mut removed = 0;
    for name in HOOK_NAMES {
        let path = dir.join(name);
        if remove_hook_block(&path)? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn install_hook_file(path: &Path) -> std::io::Result<bool> {
    let block = format!("{HOOK_START}\n{HOOK_BODY}\n{HOOK_END}\n");
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        // New hook files need a shebang so git can exec them.
        "#!/bin/sh\n".to_string()
    };

    let new_content = if existing.contains(HOOK_START) {
        let before = existing.find(HOOK_START).unwrap();
        let after = existing
            .find(HOOK_END)
            .map(|i| i + HOOK_END.len())
            .unwrap_or(existing.len());
        // Consume a trailing newline after END if present.
        let mut end = after;
        if existing[end..].starts_with('\n') {
            end += 1;
        }
        format!("{}{}{}", &existing[..before], block, &existing[end..])
    } else if existing.trim().is_empty() {
        format!("#!/bin/sh\n{block}")
    } else {
        let trimmed = existing.trim_end();
        format!("{trimmed}\n\n{block}")
    };

    if new_content == existing {
        return Ok(false);
    }
    fs::write(path, new_content)?;
    Ok(true)
}

fn remove_hook_block(path: &Path) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    if !content.contains(HOOK_START) {
        return Ok(false);
    }
    let before = content.find(HOOK_START).unwrap();
    let after = content
        .find(HOOK_END)
        .map(|i| i + HOOK_END.len())
        .unwrap_or(content.len());
    let mut end = after;
    if content[end..].starts_with('\n') {
        end += 1;
    }
    let prefix = content[..before].trim_end_matches('\n');
    let suffix = &content[end..];
    let new_content = if suffix.trim().is_empty() {
        if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}\n")
        }
    } else {
        format!("{prefix}\n{suffix}")
    };
    if new_content == content {
        return Ok(false);
    }
    if new_content.trim().is_empty() || new_content.trim() == "#!/bin/sh" {
        // Hook is empty / shebang-only — remove the file.
        fs::remove_file(path)?;
    } else {
        fs::write(path, new_content)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        dir
    }

    #[test]
    fn install_twice_is_idempotent() {
        let repo = init_repo();
        assert_eq!(install_git_hooks(repo.path()).unwrap(), 3);
        assert_eq!(install_git_hooks(repo.path()).unwrap(), 0);
        let hook = hooks_dir(repo.path()).unwrap().join("post-commit");
        let text = fs::read_to_string(&hook).unwrap();
        assert_eq!(text.matches(HOOK_START).count(), 1);
        assert!(text.contains("memlayer verify --quiet"));
        let mode = fs::metadata(&hook).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn uninstall_preserves_user_content() {
        let repo = init_repo();
        let hooks = hooks_dir(repo.path()).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        let path = hooks.join("post-commit");
        fs::write(&path, "#!/bin/sh\necho user-hook\n").unwrap();
        install_git_hooks(repo.path()).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("user-hook"));
        assert_eq!(remove_git_hooks(repo.path()).unwrap(), 3);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("user-hook"));
        assert!(!text.contains(HOOK_START));
    }
}
