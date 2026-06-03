//! Project detection: cwd → project name (PRD §9.1, FR3).
//!
//! Three cases, in order:
//!
//! 1. **Config file.** Walk up from cwd to the enclosing git root looking for
//!    `.memlayer/config.json`; return its `project_name`. Source = `config`.
//! 2. **Git remote.** If the cwd is inside a git repo with an `origin` remote,
//!    parse the remote URL (last path segment, strip `.git`).
//!    Source = `git_remote`. Supports `git@host:org/repo.git`,
//!    `https://host/org/repo[.git]`, `ssh://host/org/repo.git`.
//! 3. **Git root basename.** Git repo without `origin` → use the repo root
//!    directory name. Source = `git_root`.
//!
//! If none match, [`detect_in`] returns [`DetectError::Ambiguous`] which the
//! caller maps to exit code 5 (FR13).
//!
//! Two overrides bypass the algorithm:
//! - `--project <name>` (explicit CLI flag)
//! - `MEMLAYER_PROJECT` env var
//!
//! All names are normalized per §9.2 before being returned.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSource {
    /// `--project` flag.
    CliFlag,
    /// `MEMLAYER_PROJECT` env var.
    EnvOverride,
    /// `<repo>/.memlayer/config.json`.
    ConfigFile(PathBuf),
    /// `origin` remote URL parsed.
    GitRemote(String),
    /// Git repo root basename.
    GitRoot(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDetection {
    /// The pre-normalization name (kept for `project current --output`).
    pub display_name: String,
    /// `~/.memlayer/projects/<normalized>.db` keys off this.
    pub normalized: String,
    pub source: ProjectSource,
}

#[derive(Debug, Error)]
pub enum DetectError {
    /// Neither override, config, nor git found. Exit 5 (PRD §3.4).
    #[error(
        "memlayer: cannot determine project — current directory is not a git repo and contains no .memlayer/config.json. Either run from inside a git repo, create .memlayer/config.json with {{\"project_name\":\"<name>\"}}, or pass --project <name>."
    )]
    Ambiguous,

    #[error("invalid project name: {0}")]
    InvalidName(String),

    #[error("could not parse origin remote URL: {0}")]
    UnparseableRemote(String),

    #[error("config file: {path}: {reason}")]
    Config { path: PathBuf, reason: String },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl DetectError {
    /// Map a detection failure to a CLI exit code (FR13).
    ///
    /// `Ambiguous` and `InvalidName` are recoverable: the user can pass
    /// `--project` to bypass detection, so they map to 5. Filesystem errors
    /// and corrupt config map to 1.
    pub fn exit_code(&self) -> u8 {
        match self {
            DetectError::Ambiguous | DetectError::InvalidName(_) => 5,
            DetectError::UnparseableRemote(_) => 5,
            DetectError::Config { .. } | DetectError::Io(_) => 1,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProjectConfigFile {
    project_name: String,
}

/// Detect the project for `cwd`, honoring the two override sources.
///
/// `cli_override` is the parsed `--project` flag; `env_override` is the value
/// of `MEMLAYER_PROJECT`. Pass `None` for either when the source is absent.
pub fn detect_in(
    cwd: &Path,
    env_override: Option<String>,
    cli_override: Option<String>,
) -> Result<ProjectDetection, DetectError> {
    if let Some(name) = cli_override {
        let normalized = normalize_name(&name)?;
        return Ok(ProjectDetection {
            display_name: name,
            normalized,
            source: ProjectSource::CliFlag,
        });
    }
    if let Some(name) = env_override.filter(|s| !s.is_empty()) {
        let normalized = normalize_name(&name)?;
        return Ok(ProjectDetection {
            display_name: name,
            normalized,
            source: ProjectSource::EnvOverride,
        });
    }

    let git_root = find_git_root(cwd);

    // Case 0: config file walking up from cwd to the enclosing git root.
    if let Some(cfg_path) = find_config_in_ancestors(cwd, git_root.as_deref()) {
        let body = std::fs::read_to_string(&cfg_path)?;
        let cfg: ProjectConfigFile = serde_json::from_str(&body).map_err(|e| {
            DetectError::Config { path: cfg_path.clone(), reason: e.to_string() }
        })?;
        let normalized = normalize_name(&cfg.project_name)?;
        return Ok(ProjectDetection {
            display_name: cfg.project_name,
            normalized,
            source: ProjectSource::ConfigFile(cfg_path),
        });
    }

    let git_root = match git_root {
        Some(p) => p,
        None => return Err(DetectError::Ambiguous),
    };

    // Case 1: git remote.
    if let Some(remote) = read_origin_url(&git_root)? {
        match parse_remote_to_name(&remote) {
            Some(name) => {
                let normalized = normalize_name(&name)?;
                return Ok(ProjectDetection {
                    display_name: name,
                    normalized,
                    source: ProjectSource::GitRemote(remote),
                });
            }
            None => return Err(DetectError::UnparseableRemote(remote)),
        }
    }

    // Case 2: git root basename.
    let basename = git_root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or(DetectError::Ambiguous)?;
    let normalized = normalize_name(basename)?;
    Ok(ProjectDetection {
        display_name: basename.to_string(),
        normalized,
        source: ProjectSource::GitRoot(git_root),
    })
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur: &Path = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// Walk up from `cwd` looking for `.memlayer/config.json`. Stops at
/// `git_root` (inclusive) when present, otherwise at the filesystem root.
fn find_config_in_ancestors(cwd: &Path, git_root: Option<&Path>) -> Option<PathBuf> {
    let mut cur: &Path = cwd;
    loop {
        let cfg = cur.join(".memlayer").join("config.json");
        if cfg.is_file() {
            return Some(cfg);
        }
        if let Some(stop) = git_root {
            if cur == stop {
                return None;
            }
        }
        cur = cur.parent()?;
    }
}

/// Read `[remote "origin"] url = …` from `<git_root>/.git/config`.
fn read_origin_url(git_root: &Path) -> std::io::Result<Option<String>> {
    let cfg_path = git_root.join(".git").join("config");
    let body = match std::fs::read_to_string(&cfg_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut in_origin = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(section) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_origin = section == "remote \"origin\"";
            continue;
        }
        if in_origin {
            if let Some(rest) = trimmed.strip_prefix("url") {
                if let Some(eq) = rest.trim_start().strip_prefix('=') {
                    return Ok(Some(eq.trim().to_string()));
                }
            }
        }
    }
    Ok(None)
}

/// Last path segment of the URL, with `.git` suffix stripped.
fn parse_remote_to_name(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let last = trimmed
        .rsplit(|c: char| c == '/' || c == ':')
        .next()
        .filter(|s| !s.is_empty())?;
    let stripped = last.strip_suffix(".git").unwrap_or(last);
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

/// PRD §9.2 normalization. Returns `Err(InvalidName)` for names that begin
/// with `.` or contain `..` after normalization (path safety).
pub fn normalize_name(input: &str) -> Result<String, DetectError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DetectError::InvalidName("empty".into()));
    }
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        let lc = c.to_ascii_lowercase();
        match lc {
            ' ' | '_' => out.push('-'),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '.' => out.push(c),
            _ => {} // drop everything else
        }
    }
    if out.is_empty() {
        return Err(DetectError::InvalidName(format!("normalization removed all characters: '{input}'")));
    }
    if out.starts_with('.') {
        return Err(DetectError::InvalidName(format!("starts with '.': '{out}'")));
    }
    if out.contains("..") {
        return Err(DetectError::InvalidName(format!("contains '..': '{out}'")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal `.git/` directory with an optional `[remote "origin"]`
    /// URL. The `.git` directory is sufficient for `find_git_root`; it does
    /// not need to be a valid working git repo.
    fn make_git_repo(root: &Path, origin_url: Option<&str>) {
        fs::create_dir_all(root.join(".git")).unwrap();
        let mut cfg = String::from("[core]\nrepositoryformatversion = 0\n");
        if let Some(url) = origin_url {
            cfg.push_str(&format!("[remote \"origin\"]\n\turl = {url}\n"));
        }
        fs::write(root.join(".git").join("config"), cfg).unwrap();
    }

    fn write_config_json(repo: &Path, project_name: &str) {
        fs::create_dir_all(repo.join(".memlayer")).unwrap();
        fs::write(
            repo.join(".memlayer").join("config.json"),
            format!("{{\"project_name\":\"{project_name}\"}}"),
        )
        .unwrap();
    }

    #[test]
    fn config_json_takes_precedence() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        make_git_repo(repo, Some("git@github.com:acme/widgets.git"));
        write_config_json(repo, "Custom Project Name");

        let det = detect_in(repo, None, None).unwrap();
        assert_eq!(det.display_name, "Custom Project Name");
        assert_eq!(det.normalized, "custom-project-name");
        match det.source {
            ProjectSource::ConfigFile(_) => {}
            other => panic!("expected ConfigFile, got {other:?}"),
        }
    }

    #[test]
    fn git_remote_used_when_no_config() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path();
        make_git_repo(repo, Some("git@github.com:acme/widgets.git"));

        let det = detect_in(repo, None, None).unwrap();
        assert_eq!(det.normalized, "widgets");
        match det.source {
            ProjectSource::GitRemote(url) => assert!(url.contains("widgets")),
            other => panic!("expected GitRemote, got {other:?}"),
        }
    }

    #[test]
    fn root_basename_when_no_remote() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("My_Repo");
        fs::create_dir_all(&repo).unwrap();
        make_git_repo(&repo, None);

        let det = detect_in(&repo, None, None).unwrap();
        assert_eq!(det.normalized, "my-repo");
        match det.source {
            ProjectSource::GitRoot(_) => {}
            other => panic!("expected GitRoot, got {other:?}"),
        }
    }

    #[test]
    fn non_git_dir_returns_exit5() {
        let dir = TempDir::new().unwrap();
        let err = detect_in(dir.path(), None, None).expect_err("expected Ambiguous");
        assert!(matches!(err, DetectError::Ambiguous));
        assert_eq!(err.exit_code(), 5);
    }

    #[test]
    fn cli_override_skips_detection() {
        let dir = TempDir::new().unwrap();
        // No git, no config — would be ambiguous without override.
        let det = detect_in(dir.path(), None, Some("explicit".into())).unwrap();
        assert_eq!(det.normalized, "explicit");
        assert_eq!(det.source, ProjectSource::CliFlag);
    }

    #[test]
    fn env_override_skips_detection() {
        let dir = TempDir::new().unwrap();
        let det = detect_in(dir.path(), Some("env-name".into()), None).unwrap();
        assert_eq!(det.normalized, "env-name");
        assert_eq!(det.source, ProjectSource::EnvOverride);
    }

    #[test]
    fn cli_override_wins_over_env() {
        let dir = TempDir::new().unwrap();
        let det = detect_in(dir.path(), Some("env".into()), Some("cli".into())).unwrap();
        assert_eq!(det.normalized, "cli");
        assert_eq!(det.source, ProjectSource::CliFlag);
    }

    #[test]
    fn parse_remote_handles_https() {
        assert_eq!(parse_remote_to_name("https://github.com/acme/widgets.git"), Some("widgets".into()));
        assert_eq!(parse_remote_to_name("https://github.com/acme/widgets"), Some("widgets".into()));
    }

    #[test]
    fn parse_remote_handles_ssh() {
        assert_eq!(parse_remote_to_name("git@github.com:acme/widgets.git"), Some("widgets".into()));
        assert_eq!(parse_remote_to_name("ssh://git@github.com/acme/widgets.git"), Some("widgets".into()));
    }

    #[test]
    fn normalize_lowercases_and_replaces() {
        assert_eq!(normalize_name("My Repo_Name").unwrap(), "my-repo-name");
        assert_eq!(normalize_name("acme.api").unwrap(), "acme.api");
    }

    #[test]
    fn normalize_rejects_dotfile() {
        let err = normalize_name(".hidden").unwrap_err();
        assert!(matches!(err, DetectError::InvalidName(_)));
    }

    #[test]
    fn normalize_rejects_path_traversal() {
        let err = normalize_name("foo..bar").unwrap_err();
        assert!(matches!(err, DetectError::InvalidName(_)));
    }
}
