use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, VerifyingKey};
use flate2::read::GzDecoder;
use is_terminal::IsTerminal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::{Archive, EntryType};

use statefulmemory_core::{config, paths};

use crate::cli::{Command as CliCommand, UpdateApplyArgs, UpdateArgs, UpdateVerb};
use crate::exit;

const RELEASE_API_URL: &str =
    "https://api.github.com/repos/sanudatta11/statefulmemory/releases/latest";
const MANIFEST_URL: &str =
    "https://github.com/sanudatta11/statefulmemory/releases/latest/download/statefulmemory-update.json";
const SIGNATURE_URL: &str =
    "https://github.com/sanudatta11/statefulmemory/releases/latest/download/statefulmemory-update.json.sig";
const UPDATE_PUBLIC_KEY_HEX: &str =
    "afff9bdc391221a753b80114bc49bba53d4a0f5c6762c55c49b5d5f83532f20b";
const USER_AGENT: &str = concat!("statefulmemory/", env!("CARGO_PKG_VERSION"));
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 16 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 160 * 1024 * 1024;
const MAX_BINARY_BYTES: usize = 120 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct UpdateManifest {
    version: String,
    #[serde(default)]
    releases: BTreeMap<String, ManifestArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestArtifact {
    url: String,
    sha256: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UpdateState {
    last_checked: u64,
    latest_version: Option<String>,
    notified_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdatePlan {
    parent_pid: u32,
    version: String,
    previous_version: String,
    staged_binary: PathBuf,
    targets: Vec<TargetPlan>,
    #[serde(default)]
    rollback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TargetPlan {
    path: PathBuf,
    backup: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim().strip_prefix('v').unwrap_or(value.trim());
        let value = value.split('-').next().unwrap_or(value);
        let mut parts = value.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

pub fn should_auto_update(command: &CliCommand) -> bool {
    !matches!(
        command,
        CliCommand::Update(_)
            | CliCommand::Version
            | CliCommand::Mcp
            | CliCommand::Hook(_)
            | CliCommand::Daemon(_)
            | CliCommand::UpdateCheck
            | CliCommand::UpdateApply(_)
            | CliCommand::UpdateRollback(_)
    )
}

pub fn maybe_auto_update() {
    if ci_environment() || development_executable() {
        return;
    }
    let cfg = config::load_resolved(None).update;
    if !cfg.enabled || env_disabled("STATEFULMEMORY_UPDATE_CHECK") || is_homebrew_managed() {
        return;
    }
    notify_if_available();
    if check_due(cfg.check_interval_hours) {
        spawn_background_check();
    }
}

pub fn dispatch(args: UpdateArgs) -> ExitCode {
    let result = match args.verb.as_ref() {
        Some(UpdateVerb::Rollback) => rollback(args.yes),
        Some(UpdateVerb::Check) => check_command(),
        None if args.check => check_command(),
        None => install_command(args.yes),
    };
    match result {
        Ok(message) => {
            if !message.is_empty() {
                println!("{message}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("statefulmemory update: {error}");
            ExitCode::from(exit::GENERAL)
        }
    }
}

pub fn dispatch_check_helper() -> ExitCode {
    let result = fetch_latest_version(Duration::from_secs(10)).and_then(|version| {
        let now = now_seconds();
        let mut state = read_state();
        state.last_checked = now;
        state.latest_version = Some(version);
        write_state(&state)
    });
    if result.is_err() {
        let mut state = read_state();
        state.last_checked = now_seconds();
        let _ = write_state(&state);
        return ExitCode::from(exit::GENERAL);
    }
    ExitCode::SUCCESS
}

pub fn dispatch_apply_helper(args: UpdateApplyArgs) -> ExitCode {
    match apply_helper(&args.plan) {
        Ok(message) => {
            if !message.is_empty() {
                println!("{message}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("statefulmemory updater: {error}");
            ExitCode::from(exit::GENERAL)
        }
    }
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
    })
}

fn check_command() -> Result<String, String> {
    let (latest_version, signed) = match fetch_manifest(Duration::from_secs(20)) {
        Ok(manifest) => (manifest.version, true),
        Err(_) => (fetch_latest_version(Duration::from_secs(20))?, false),
    };
    let latest = Version::parse(&latest_version)
        .ok_or_else(|| format!("invalid update version: {latest_version}"))?;
    let current = current_version();
    println!("Current version: {}", env!("CARGO_PKG_VERSION"));
    println!("Latest version:  {latest_version}");
    if latest <= current {
        Ok("StatefulMemory is up to date.".into())
    } else if signed {
        Ok(format!(
            "Run `statefulmemory update` to install {latest_version}."
        ))
    } else {
        Ok(format!(
            "{latest_version} is available, but its signed update manifest is not published yet."
        ))
    }
}

fn install_command(yes: bool) -> Result<String, String> {
    if let Some(manager) = package_manager() {
        return Err(format!(
            "StatefulMemory appears to be managed by {manager}; run `{manager} upgrade statefulmemory`"
        ));
    }
    let latest_tag = fetch_latest_version(Duration::from_secs(20))?;
    let latest_from_api = Version::parse(&latest_tag)
        .ok_or_else(|| format!("invalid update version: {latest_tag}"))?;
    if latest_from_api <= current_version() {
        return Ok("StatefulMemory is up to date.".into());
    }
    let manifest = fetch_manifest(Duration::from_secs(20))?;
    let latest = Version::parse(&manifest.version)
        .ok_or_else(|| format!("invalid update version: {}", manifest.version))?;
    if latest <= current_version() {
        return Ok("StatefulMemory is up to date.".into());
    }
    println!("Current version: {}", env!("CARGO_PKG_VERSION"));
    println!("Latest version:  {}", manifest.version);
    if !yes {
        if !io::stdin().is_terminal() {
            return Err("confirmation requires a terminal; rerun with --yes".into());
        }
        if !confirm("Download and install this update? [y/N] ") {
            return Ok("Update cancelled.".into());
        }
    }
    let plan = prepare_install(&manifest)?;
    spawn_apply_helper(&plan)?;
    Ok(format!(
        "Update {} downloaded and verified. The updater will finish after this process exits.",
        manifest.version
    ))
}

fn fetch_manifest(timeout: Duration) -> Result<UpdateManifest, String> {
    let manifest_bytes = download(MANIFEST_URL, MAX_MANIFEST_BYTES, timeout)?;
    let signature_bytes = download(SIGNATURE_URL, MAX_SIGNATURE_BYTES, timeout)?;
    verify_manifest(&manifest_bytes, &signature_bytes)
}

fn fetch_latest_version(timeout: Duration) -> Result<String, String> {
    let bytes = download(RELEASE_API_URL, MAX_MANIFEST_BYTES, timeout)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid GitHub release response: {error}"))?;
    value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "GitHub release response has no tag_name".to_string())
}

fn verify_manifest(bytes: &[u8], signature: &[u8]) -> Result<UpdateManifest, String> {
    let key_bytes: [u8; 32] = decode_hex(UPDATE_PUBLIC_KEY_HEX)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "embedded update public key is invalid".to_string())?;
    let key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|error| format!("embedded update public key is invalid: {error}"))?;
    verify_manifest_with_key(bytes, signature, &key)
}

fn verify_manifest_with_key(
    bytes: &[u8],
    signature: &[u8],
    key: &VerifyingKey,
) -> Result<UpdateManifest, String> {
    let signature_bytes = decode_signature(signature)
        .ok_or_else(|| "update manifest signature is not valid hex or raw bytes".to_string())?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|error| format!("invalid update manifest signature: {error}"))?;
    key.verify_strict(bytes, &signature)
        .map_err(|error| format!("update manifest signature verification failed: {error}"))?;
    serde_json::from_slice(bytes).map_err(|error| format!("invalid update manifest: {error}"))
}

fn prepare_install(manifest: &UpdateManifest) -> Result<UpdatePlan, String> {
    let target = target_key()?;
    let artifact = manifest
        .releases
        .get(&target)
        .ok_or_else(|| format!("signed manifest has no artifact for {target}"))?;
    if !artifact.url.starts_with("https://") {
        return Err("update artifact URL must use HTTPS".into());
    }
    if artifact.sha256.len() != 64 || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("signed manifest contains an invalid SHA-256".into());
    }
    let max = if artifact.size == 0 {
        MAX_DOWNLOAD_BYTES
    } else {
        artifact.size.min(MAX_DOWNLOAD_BYTES)
    };
    let archive = download(&artifact.url, max, Duration::from_secs(300))?;
    verify_checksum(&archive, artifact.sha256.to_ascii_lowercase())?;
    let binary = extract_binary(&archive)?;
    if binary.len() > MAX_BINARY_BYTES {
        return Err("release binary is too large".into());
    }
    let update_dir = update_dir()?;
    fs::create_dir_all(&update_dir)
        .map_err(|error| format!("create {}: {error}", update_dir.display()))?;
    let staged = update_dir.join(format!(
        "staged-{}-{}",
        manifest.version,
        std::process::id()
    ));
    fs::write(&staged, &binary).map_err(|error| format!("write {}: {error}", staged.display()))?;
    set_executable(&staged)?;
    verify_binary(&staged, &manifest.version)?;
    let targets = executable_targets()?;
    let targets = targets
        .into_iter()
        .map(|path| TargetPlan {
            backup: path.with_file_name(format!(
                ".{}.backup-{}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("statefulmemory"),
                manifest.version
            )),
            path,
        })
        .collect();
    let plan_path = update_dir.join(format!("plan-{}.json", std::process::id()));
    let plan = UpdatePlan {
        parent_pid: std::process::id(),
        version: manifest.version.clone(),
        previous_version: env!("CARGO_PKG_VERSION").to_string(),
        staged_binary: staged,
        targets,
        rollback: false,
    };
    let plan_bytes = serde_json::to_vec_pretty(&plan)
        .map_err(|error| format!("serialize update plan: {error}"))?;
    fs::write(&plan_path, plan_bytes)
        .map_err(|error| format!("write {}: {error}", plan_path.display()))?;
    Ok(UpdatePlan {
        parent_pid: plan.parent_pid,
        version: plan.version,
        previous_version: plan.previous_version,
        staged_binary: plan.staged_binary,
        targets: plan.targets,
        rollback: false,
    })
}

fn spawn_apply_helper(plan: &UpdatePlan) -> Result<(), String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("could not locate updater executable: {error}"))?;
    let plan_path = plan
        .staged_binary
        .parent()
        .ok_or_else(|| "staged update has no parent directory".to_string())?
        .join(format!("plan-{}.json", plan.parent_pid));
    let mut command = ProcessCommand::new(current);
    command
        .arg("__update-apply")
        .arg("--plan")
        .arg(&plan_path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start updater helper: {error}"))
}

fn apply_helper(plan_path: &Path) -> Result<String, String> {
    let plan: UpdatePlan = serde_json::from_slice(
        &fs::read(plan_path).map_err(|error| format!("read update plan: {error}"))?,
    )
    .map_err(|error| format!("invalid update plan: {error}"))?;
    wait_for_parent(plan.parent_pid)?;
    if plan.rollback {
        return apply_rollback(&plan, plan_path);
    }
    let mut installed = Vec::new();
    for target in &plan.targets {
        let backup = &target.backup;
        if backup.exists() {
            fs::remove_file(backup)
                .map_err(|error| format!("remove old backup {}: {error}", backup.display()))?;
        }
        if target.path.exists() {
            fs::rename(&target.path, backup)
                .map_err(|error| format!("backup {}: {error}", target.path.display()))?;
        }
        match install_staged(&plan.staged_binary, &target.path, &plan.version) {
            Ok(()) => installed.push(target),
            Err(error) => {
                rollback_targets(&installed);
                if backup.exists() {
                    let _ = fs::rename(backup, &target.path);
                }
                let _ = fs::remove_file(&plan.staged_binary);
                let _ = fs::remove_file(plan_path);
                return Err(error);
            }
        }
    }
    let rollback = serde_json::to_vec_pretty(&plan)
        .map_err(|error| format!("serialize rollback record: {error}"))?;
    let rollback_dir = update_dir()?;
    fs::create_dir_all(&rollback_dir).map_err(|error| {
        format!(
            "create rollback directory {}: {error}",
            rollback_dir.display()
        )
    })?;
    let rollback_path = rollback_dir.join("last-update.json");
    fs::write(&rollback_path, rollback)
        .map_err(|error| format!("write rollback record: {error}"))?;
    let _ = fs::remove_file(&plan.staged_binary);
    let _ = fs::remove_file(plan_path);
    let _ = remove_check_time();
    Ok(format!("StatefulMemory updated to {}.", plan.version))
}

fn apply_rollback(plan: &UpdatePlan, plan_path: &Path) -> Result<String, String> {
    let mut current_backups = Vec::new();
    for target in &plan.targets {
        if !target.backup.exists() {
            return Err(format!(
                "rollback backup is missing: {}",
                target.backup.display()
            ));
        }
        let parent = target
            .path
            .parent()
            .ok_or_else(|| format!("binary target has no parent: {}", target.path.display()))?;
        let name = target
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid binary target: {}", target.path.display()))?;
        let current = parent.join(format!(".{name}.pre-rollback-{}", std::process::id()));
        let temp = parent.join(format!(".{name}.rollback-{}", std::process::id()));
        if target.path.exists() {
            fs::rename(&target.path, &current)
                .map_err(|error| format!("backup current {}: {error}", target.path.display()))?;
            current_backups.push((current.clone(), target.path.clone()));
        }
        let result = fs::copy(&target.backup, &temp)
            .map_err(|error| format!("copy rollback {}: {error}", temp.display()))
            .and_then(|_| set_executable(&temp))
            .and_then(|_| {
                fs::rename(&temp, &target.path)
                    .map_err(|error| format!("install rollback {}: {error}", target.path.display()))
            })
            .and_then(|_| verify_binary(&target.path, &plan.previous_version));
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            if current.exists() {
                let _ = fs::rename(&current, &target.path);
            }
            for (backup, target) in current_backups.iter().rev() {
                let _ = fs::remove_file(target);
                if backup.exists() {
                    let _ = fs::rename(backup, target);
                }
            }
            let _ = fs::remove_file(plan_path);
            return Err(error);
        }
        let _ = fs::remove_file(&current);
    }
    let rollback_record = update_dir()?.join("last-update.json");
    let _ = fs::remove_file(rollback_record);
    let _ = fs::remove_file(plan_path);
    Ok(format!(
        "StatefulMemory rolled back to {}.",
        plan.previous_version
    ))
}

fn install_staged(staged: &Path, target: &Path, version: &str) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("binary target has no parent: {}", target.display()))?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid binary target: {}", target.display()))?;
    let temp = parent.join(format!(".{name}.new-{}", std::process::id()));
    fs::copy(staged, &temp).map_err(|error| format!("copy {}: {error}", temp.display()))?;
    set_executable(&temp)?;
    fs::rename(&temp, target).map_err(|error| format!("install {}: {error}", target.display()))?;
    verify_binary(target, version)
}

fn rollback_targets(targets: &[&TargetPlan]) {
    for target in targets {
        let _ = fs::remove_file(&target.path);
        if target.backup.exists() {
            let _ = fs::rename(&target.backup, &target.path);
        }
    }
}

fn rollback(yes: bool) -> Result<String, String> {
    let path = update_dir()?.join("last-update.json");
    let plan: UpdatePlan = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("no previous standalone update: {error}"))?,
    )
    .map_err(|error| format!("invalid rollback record: {error}"))?;
    if !yes {
        if !io::stdin().is_terminal() {
            return Err("confirmation requires a terminal; rerun with --yes".into());
        }
        if !confirm("Restore the previous StatefulMemory binary? [y/N] ") {
            return Ok("Rollback cancelled.".into());
        }
    }
    let rollback_plan = UpdatePlan {
        parent_pid: std::process::id(),
        version: plan.version,
        previous_version: plan.previous_version,
        staged_binary: PathBuf::new(),
        targets: plan.targets,
        rollback: true,
    };
    let plan_path = update_dir()?.join(format!("rollback-plan-{}.json", std::process::id()));
    fs::write(
        &plan_path,
        serde_json::to_vec_pretty(&rollback_plan)
            .map_err(|error| format!("serialize rollback plan: {error}"))?,
    )
    .map_err(|error| format!("write rollback plan: {error}"))?;
    spawn_named_helper("__update-rollback", &plan_path)?;
    Ok("Rollback scheduled after this process exits.".into())
}

fn spawn_named_helper(name: &str, plan_path: &Path) -> Result<(), String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("could not locate updater executable: {error}"))?;
    ProcessCommand::new(current)
        .arg(name)
        .arg("--plan")
        .arg(plan_path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start updater helper: {error}"))
}

fn wait_for_parent(pid: u32) -> Result<(), String> {
    for _ in 0..300 {
        if !process_exists(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("timed out waiting for parent process to exit".into())
}

fn process_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(raw_pid) = i32::try_from(pid) else {
        return false;
    };
    let pid = nix::unistd::Pid::from_raw(raw_pid);
    match nix::sys::signal::kill(pid, None) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => true,
    }
}

fn executable_targets() -> Result<Vec<PathBuf>, String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("could not locate running executable: {error}"))?;
    let current = fs::canonicalize(&current).unwrap_or(current);
    let parent = current
        .parent()
        .ok_or_else(|| "running executable has no parent directory".to_string())?;
    let mut targets = Vec::new();
    for name in ["statefulmemory", "smem", "sm"] {
        let candidate = parent.join(name);
        if candidate.exists() {
            let target = fs::canonicalize(&candidate).unwrap_or(candidate);
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    if targets.is_empty() {
        targets.push(current);
    }
    Ok(targets)
}

fn verify_binary(path: &Path, version: &str) -> Result<(), String> {
    let output = ProcessCommand::new(path)
        .arg("--version")
        .output()
        .map_err(|error| format!("verify {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "new binary failed version check: {}",
            path.display()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.contains(version.trim_start_matches('v')) {
        return Err(format!(
            "downloaded binary reports an unexpected version: {text}"
        ));
    }
    Ok(())
}

fn is_homebrew_managed() -> bool {
    std::env::current_exe()
        .ok()
        .map(|path| {
            let text = fs::canonicalize(&path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_ascii_lowercase();
            text.contains("/cellar/") || text.contains("/homebrew/") || text.contains("linuxbrew")
        })
        .unwrap_or(false)
}

fn package_manager() -> Option<&'static str> {
    let path = std::env::current_exe().ok()?;
    let path = fs::canonicalize(&path).unwrap_or(path);
    let text = path.to_string_lossy().to_ascii_lowercase();
    if text.contains("/cellar/") || text.contains("/homebrew/") || text.contains("linuxbrew") {
        return Some("brew");
    }
    if text.starts_with("/usr/bin/") || text.starts_with("/bin/") {
        let package_query = ProcessCommand::new("dpkg-query")
            .arg("-S")
            .arg(path)
            .output();
        if package_query.is_ok_and(|output| output.status.success()) {
            return Some("apt");
        }
    }
    None
}

fn target_key() -> Result<String, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") | ("darwin", "aarch64") => Ok("darwin-arm64".into()),
        ("macos", "x86_64") | ("darwin", "x86_64") => Ok("darwin-amd64".into()),
        ("linux", "x86_64") => Ok("linux-amd64".into()),
        ("linux", "aarch64") => Ok("linux-arm64".into()),
        (os, arch) => Err(format!("unsupported update platform: {os}/{arch}")),
    }
}

fn extract_binary(archive_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let decoder = GzDecoder::new(io::Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("read release archive: {error}"))?;
    let mut binary = None;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("read release archive entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("read release archive path: {error}"))?
            .into_owned();
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("release archive contains an unsafe path".into());
        }
        match entry.header().entry_type() {
            EntryType::Directory => {}
            EntryType::Regular => {
                if path.file_name().and_then(|name| name.to_str()) == Some("statefulmemory") {
                    if binary.is_some() {
                        return Err("release archive contains multiple binaries".into());
                    }
                    let mut bytes = Vec::new();
                    entry
                        .read_to_end(&mut bytes)
                        .map_err(|error| format!("read release binary: {error}"))?;
                    binary = Some(bytes);
                }
            }
            _ => return Err("release archive contains an unsupported entry".into()),
        }
    }
    binary.ok_or_else(|| "release archive has no statefulmemory binary".into())
}

fn download(url: &str, max_bytes: u64, timeout: Duration) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout.min(Duration::from_secs(15)))
        .timeout_read(timeout)
        .build();
    let accept = if url == RELEASE_API_URL {
        "application/vnd.github+json"
    } else {
        "application/octet-stream"
    };
    let response = agent
        .get(url)
        .set("Accept", accept)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|error| format!("download failed: {error}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("download read failed: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err("download exceeds size limit".into());
    }
    Ok(bytes)
}

fn verify_checksum(bytes: &[u8], expected: String) -> Result<(), String> {
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch: expected {expected}, got {actual}"
        ))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn decode_signature(value: &[u8]) -> Option<Vec<u8>> {
    if let Ok(text) = std::str::from_utf8(value) {
        let text = text.trim();
        if text.len() % 2 == 0 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return decode_hex(text);
        }
    }
    Some(value.to_vec())
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|error| format!("stdout: {error}"))
        .unwrap_or_default();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn update_dir() -> Result<PathBuf, String> {
    Ok(paths::data_dir().join("update"))
}

fn state_path() -> PathBuf {
    paths::data_dir().join("update-check.json")
}

fn read_state() -> UpdateState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}

fn write_state(state: &UpdateState) -> Result<(), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("serialize update state: {error}"))?;
    fs::write(&temp, bytes).map_err(|error| format!("write {}: {error}", temp.display()))?;
    fs::rename(&temp, &path).map_err(|error| format!("write {}: {error}", path.display()))
}

fn check_due(interval_hours: u64) -> bool {
    let state = read_state();
    now_seconds().saturating_sub(state.last_checked) >= interval_hours.saturating_mul(60 * 60)
}

fn notify_if_available() {
    let mut state = read_state();
    let Some(latest) = state.latest_version.as_deref() else {
        return;
    };
    let Some(latest_version) = Version::parse(latest) else {
        return;
    };
    if latest_version <= current_version() || state.notified_version.as_deref() == Some(latest) {
        return;
    }
    eprintln!(
        "Update available: {} -> {}. Run `statefulmemory update` to install it.",
        env!("CARGO_PKG_VERSION"),
        latest
    );
    state.notified_version = Some(latest.to_string());
    let _ = write_state(&state);
}

fn spawn_background_check() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let _ = ProcessCommand::new(executable)
        .arg("__update-check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn remove_check_time() -> io::Result<()> {
    match fs::remove_file(state_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn ci_environment() -> bool {
    ["CI", "GITHUB_ACTIONS", "BUILDKITE", "GITLAB_CI"]
        .iter()
        .any(|name| env_disabled(name))
}

fn env_disabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn development_executable() -> bool {
    std::env::current_exe()
        .ok()
        .map(|path| {
            let path = path.to_string_lossy();
            path.contains("/target/debug/")
                || path.contains("/target/release/")
                || path.contains("\\target\\debug\\")
                || path.contains("\\target\\release\\")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use flate2::{write::GzEncoder, Compression};
    use tar::{Builder, Header};

    #[test]
    fn version_comparison_ignores_v_prefix_and_prerelease() {
        assert_eq!(
            Version::parse("v1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert!(Version::parse("v1.2.4") > Version::parse("1.2.3"));
        assert_eq!(Version::parse("1.2.3-beta"), Version::parse("1.2.3"));
    }

    #[test]
    fn manifest_signature_is_verified_before_parsing() {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let manifest = br#"{"version":"1.2.3","releases":{}}"#;
        let signature = signing_key.sign(manifest);
        let parsed = verify_manifest_with_key(
            manifest,
            &signature.to_bytes(),
            &signing_key.verifying_key(),
        )
        .unwrap();
        assert_eq!(parsed.version, "1.2.3");
    }

    #[test]
    #[cfg(unix)]
    fn helper_install_and_rollback_restore_target() {
        use std::ffi::OsString;

        struct DataDirReset(Option<OsString>);
        impl Drop for DataDirReset {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    std::env::set_var("STATEFULMEMORY_DATA_DIR", value);
                } else {
                    std::env::remove_var("STATEFULMEMORY_DATA_DIR");
                }
            }
        }

        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let _reset = DataDirReset(std::env::var_os("STATEFULMEMORY_DATA_DIR"));
        std::env::set_var("STATEFULMEMORY_DATA_DIR", &data_dir);
        let target = temp.path().join("statefulmemory");
        let backup = temp.path().join(".statefulmemory.backup-9.9.9");
        let staged = temp.path().join("staged");
        let old_script = b"#!/bin/sh\nprintf '0.1.0\\n'\n";
        let new_script = b"#!/bin/sh\nprintf '9.9.9\\n'\n";
        fs::write(&target, old_script).unwrap();
        fs::write(&staged, new_script).unwrap();
        set_executable(&target).unwrap();
        set_executable(&staged).unwrap();
        let target_plan = TargetPlan {
            path: target.clone(),
            backup: backup.clone(),
        };
        let plan = UpdatePlan {
            parent_pid: 0,
            version: "9.9.9".into(),
            previous_version: "0.1.0".into(),
            staged_binary: staged.clone(),
            targets: vec![target_plan.clone()],
            rollback: false,
        };
        let plan_path = temp.path().join("plan.json");
        fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
        assert!(apply_helper(&plan_path).unwrap().contains("9.9.9"));
        assert!(backup.exists());
        assert!(!staged.exists());
        assert!(data_dir.join("update/last-update.json").exists());

        let rollback_plan = UpdatePlan {
            parent_pid: 0,
            version: plan.version,
            previous_version: plan.previous_version,
            staged_binary: PathBuf::new(),
            targets: vec![target_plan],
            rollback: true,
        };
        let rollback_path = temp.path().join("rollback-plan.json");
        fs::write(&rollback_path, serde_json::to_vec(&rollback_plan).unwrap()).unwrap();
        assert!(apply_rollback(&rollback_plan, &rollback_path)
            .unwrap()
            .contains("0.1.0"));
        assert_eq!(fs::read(&target).unwrap(), old_script);
        assert!(!data_dir.join("update/last-update.json").exists());
    }

    #[test]
    fn archive_extraction_returns_the_packaged_binary() {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "statefulmemory", &[1_u8][..])
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&archive).unwrap();
        let bytes = encoder.finish().unwrap();
        assert_eq!(extract_binary(&bytes).unwrap(), vec![1]);
    }
}
