//! `statefulmemory uninstall` / `smem uninstall` / `sm uninstall` — remove skill
//! files, settings patches, and MCP registration. With `--purge`, also wipe data
//! dirs, model caches, binaries, and legacy `memlayer` residue.

use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::formatter::Formatter;

pub async fn dispatch_uninstall(_fmt: Formatter, purge: bool) -> ExitCode {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("statefulmemory uninstall: could not determine home directory");
            return ExitCode::from(1);
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());

    if purge {
        return dispatch_purge(&home, &cwd).await;
    }

    let file_targets: Vec<PathBuf> = vec![
        home.join(".claude")
            .join("skills")
            .join("statefulmemory")
            .join("SKILL.md"),
        cwd.join(".claude")
            .join("skills")
            .join("statefulmemory")
            .join("SKILL.md"),
        home.join(".codeium")
            .join("windsurf")
            .join("rules")
            .join("statefulmemory-memory.md"),
        cwd.join(".windsurf")
            .join("rules")
            .join("statefulmemory-memory.md"),
        home.join(".cursor").join("rules").join("statefulmemory.mdc"),
        PathBuf::from("/usr/local/bin/statefulmemory"),
    ];
    let block_targets: Vec<PathBuf> = vec![
        home.join("CLAUDE.md"),
        home.join(".github").join("copilot-instructions.md"),
    ];
    let settings_path = home.join(".claude").join("settings.json");
    let existing_mcp = crate::mcp_install::existing_registrations(&home, &cwd);
    let git_hooks_present = crate::git_hooks::hooks_dir(&cwd)
        .map(|d| {
            ["post-commit", "post-merge", "post-checkout"]
                .iter()
                .any(|n| {
                    d.join(n)
                        .exists()
                        .then(|| fs::read_to_string(d.join(n)).unwrap_or_default())
                        .is_some_and(|t| t.contains(crate::git_hooks::HOOK_START))
                })
        })
        .unwrap_or(false);

    let existing_files: Vec<&PathBuf> = file_targets.iter().filter(|p| p.exists()).collect();
    let existing_blocks: Vec<&PathBuf> = block_targets.iter().filter(|p| p.exists()).collect();

    if existing_files.is_empty()
        && existing_blocks.is_empty()
        && existing_mcp.is_empty()
        && !settings_path.exists()
        && !git_hooks_present
    {
        println!("Nothing to uninstall — no statefulmemory files found.");
        return ExitCode::SUCCESS;
    }

    println!("The following will be removed:");
    for p in &existing_files {
        println!("  delete  {}", p.display());
    }
    for p in &existing_blocks {
        println!("  remove block from  {}", p.display());
    }
    for (label, path) in &existing_mcp {
        println!("  unregister MCP ({label})  {}", path.display());
    }
    if settings_path.exists() {
        println!("  undo patches in  {}", settings_path.display());
    }
    if git_hooks_present {
        println!(
            "  remove statefulmemory block from  .git/hooks/{{post-commit,post-merge,post-checkout}}"
        );
    }
    println!();

    if !confirm("Proceed with uninstall? [y/N] ") {
        println!("Aborted.");
        return ExitCode::SUCCESS;
    }

    let mut removed: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    for path in &existing_files {
        match fs::remove_file(path) {
            Ok(()) => removed.push(format!("deleted  {}", path.display())),
            Err(e) => failed.push(format!("failed   {} ({e})", path.display())),
        }
    }

    for path in &existing_blocks {
        match remove_statefulmemory_block(path) {
            Ok(true) => removed.push(format!("unblocked  {}", path.display())),
            Ok(false) => {}
            Err(e) => failed.push(format!("failed   {} ({e})", path.display())),
        }
    }

    for result in crate::mcp_install::uninstall_all(&home, &cwd) {
        match result {
            Ok(action) => removed.push(format!(
                "unregistered MCP ({})  {}",
                action.label,
                action.path.display()
            )),
            Err((label, path, err)) => {
                failed.push(format!("failed   {label} {} ({err})", path.display()))
            }
        }
    }

    if settings_path.exists() {
        match unpatch_claude_settings(&settings_path) {
            Ok(true) => removed.push(format!("unpatched  {}", settings_path.display())),
            Ok(false) => {}
            Err(e) => failed.push(format!("failed   {} ({e})", settings_path.display())),
        }
    }

    if git_hooks_present {
        match crate::git_hooks::remove_git_hooks(&cwd) {
            Ok(n) if n > 0 => {
                removed.push(format!("removed statefulmemory blocks from {n} git hook(s)"))
            }
            Ok(_) => {}
            Err(e) => failed.push(format!("failed   git hooks ({e})")),
        }
    }

    if !removed.is_empty() {
        println!("Removed:");
        for line in &removed {
            println!("  ✓ {line}");
        }
    }
    if !failed.is_empty() {
        println!("Failed:");
        for line in &failed {
            println!("  ✗ {line}");
        }
        return ExitCode::from(1);
    }

    println!();
    println!("Uninstall complete. Restart your agent to take effect.");
    ExitCode::SUCCESS
}

async fn dispatch_purge(home: &Path, cwd: &Path) -> ExitCode {
    let _ = std::process::Command::new("statefulmemory")
        .args(["daemon", "stop"])
        .status();
    let _ = std::process::Command::new("smem")
        .args(["daemon", "stop"])
        .status();
    let _ = std::process::Command::new("sm")
        .args(["daemon", "stop"])
        .status();
    let _ = std::process::Command::new("memlayer")
        .args(["daemon", "stop"])
        .status();

    // Best-effort stop of the install-managed Laya sidecar before wiping data.
    stop_laya_sidecar(home);

    let data_dirs = [
        home.join(".statefulmemory"),
        home.join(".memlayer"),
        cwd.join(".statefulmemory"),
        cwd.join(".memlayer"),
    ];
    let model_dirs = [
        home.join(".statefulmemory-models"),
        home.join(".memlayer-models"),
    ];
    let bins = [
        home.join(".local/bin/statefulmemory"),
        home.join(".local/bin/smem"),
        home.join(".local/bin/sm"),
        home.join(".local/bin/memlayer"),
        PathBuf::from("/usr/local/bin/statefulmemory"),
        PathBuf::from("/usr/local/bin/smem"),
        PathBuf::from("/usr/local/bin/sm"),
        PathBuf::from("/usr/local/bin/memlayer"),
    ];
    let skill_dirs = [
        home.join(".claude/skills/statefulmemory"),
        home.join(".claude/skills/memlayer"),
        cwd.join(".claude/skills/statefulmemory"),
        cwd.join(".claude/skills/memlayer"),
        home.join(".agents/skills/statefulmemory"),
        home.join(".agents/skills/memlayer"),
        cwd.join(".agents/skills/statefulmemory"),
        cwd.join(".agents/skills/memlayer"),
    ];

    println!("⚠️  PURGE — permanently delete legacy memlayer + statefulmemory residue:");
    for d in data_dirs
        .iter()
        .chain(model_dirs.iter())
        .chain(skill_dirs.iter())
    {
        if d.exists() {
            println!("  dir   {}", d.display());
        }
    }
    for b in &bins {
        if b.exists() {
            println!("  bin   {}", b.display());
        }
    }
    println!("  MCP entries keyed memlayer / statefulmemory");
    println!("  git hook markers for both brands");
    println!();
    println!("This cannot be undone.");
    println!();

    if !confirm("Type 'yes' to confirm purge: ") {
        println!("Aborted.");
        return ExitCode::SUCCESS;
    }

    let mut removed = Vec::new();
    let mut failed = Vec::new();

    for d in data_dirs
        .iter()
        .chain(model_dirs.iter())
        .chain(skill_dirs.iter())
    {
        if !d.exists() {
            continue;
        }
        match fs::remove_dir_all(d) {
            Ok(()) => removed.push(format!("deleted dir  {}", d.display())),
            Err(e) => failed.push(format!("failed dir   {} ({e})", d.display())),
        }
    }
    for b in &bins {
        if !b.exists() {
            continue;
        }
        match fs::remove_file(b) {
            Ok(()) => removed.push(format!("deleted bin  {}", b.display())),
            Err(e) => failed.push(format!("failed bin   {} ({e})", b.display())),
        }
    }

    for result in crate::mcp_install::uninstall_all(home, cwd) {
        match result {
            Ok(action) => removed.push(format!(
                "unregistered MCP ({})  {}",
                action.label,
                action.path.display()
            )),
            Err((label, path, err)) => {
                failed.push(format!("failed MCP {label} {} ({err})", path.display()))
            }
        }
    }
    strip_legacy_mcp_keys(home, cwd, &mut removed, &mut failed);

    let _ = crate::git_hooks::remove_git_hooks(cwd);
    remove_legacy_hook_markers(cwd, &mut removed);

    let settings_path = home.join(".claude").join("settings.json");
    if settings_path.exists() {
        let _ = unpatch_claude_settings(&settings_path);
        let _ = unpatch_legacy_claude_settings(&settings_path);
    }

    if !removed.is_empty() {
        println!("Purged:");
        for line in &removed {
            println!("  ✓ {line}");
        }
    }
    if !failed.is_empty() {
        println!("Failed:");
        for line in &failed {
            println!("  ✗ {line}");
        }
        return ExitCode::from(1);
    }

    println!();
    println!("Purge complete. Install fresh with:");
    println!("  cargo install --path crates/statefulmemory-cli");
    println!("  smem install");
    ExitCode::SUCCESS
}

fn strip_legacy_mcp_keys(
    home: &Path,
    cwd: &Path,
    removed: &mut Vec<String>,
    failed: &mut Vec<String>,
) {
    let candidates = [
        home.join(".cursor/mcp.json"),
        cwd.join(".cursor/mcp.json"),
        home.join(".codeium/windsurf/mcp_config.json"),
        home.join(".config/opencode/opencode.json"),
        home.join(".claude.json"),
    ];
    for path in candidates {
        if !path.exists() {
            continue;
        }
        match remove_json_server_key(&path, "memlayer") {
            Ok(true) => removed.push(format!("stripped memlayer MCP key  {}", path.display())),
            Ok(false) => {}
            Err(e) => failed.push(format!("failed MCP strip {} ({e})", path.display())),
        }
    }
}

fn remove_json_server_key(path: &Path, key: &str) -> std::io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let mut root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let mut changed = false;
    for pointer in ["/mcpServers", "/servers", "/mcp", "/mcp/servers"] {
        if let Some(map) = root.pointer_mut(pointer).and_then(|v| v.as_object_mut()) {
            if map.remove(key).is_some() {
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(false);
    }
    fs::write(
        path,
        serde_json::to_string_pretty(&root).map_err(std::io::Error::other)? + "\n",
    )?;
    Ok(true)
}

fn remove_legacy_hook_markers(cwd: &Path, removed: &mut Vec<String>) {
    let Some(hooks) = crate::git_hooks::hooks_dir(cwd) else {
        return;
    };
    const LEGACY_START: &str = "# >>> memlayer >>>";
    const LEGACY_END: &str = "# <<< memlayer <<<";
    for name in ["post-commit", "post-merge", "post-checkout"] {
        let path = hooks.join(name);
        if !path.exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains(LEGACY_START) {
            continue;
        }
        let before = content.find(LEGACY_START).unwrap();
        let after = content
            .find(LEGACY_END)
            .map(|i| i + LEGACY_END.len())
            .unwrap_or(content.len());
        let new_content = format!("{}{}", &content[..before], content[after..].trim_start());
        if fs::write(&path, new_content).is_ok() {
            removed.push(format!(
                "removed legacy memlayer hook block from {}",
                path.display()
            ));
        }
    }
}

fn unpatch_legacy_claude_settings(path: &Path) -> std::io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let mut root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let original = root.clone();
    if let Some(arr) = root
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some("Bash(memlayer *)"));
    }
    if let Some(arr) = root
        .pointer_mut("/sandbox/network/allowUnixSockets")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some("~/.memlayer/daemon.sock"));
    }
    if root == original {
        return Ok(false);
    }
    fs::write(
        path,
        serde_json::to_string_pretty(&root).map_err(std::io::Error::other)? + "\n",
    )?;
    Ok(true)
}

pub async fn dispatch_clean(_fmt: Formatter) -> ExitCode {
    let data_dir = statefulmemory_core::paths::data_dir();

    if !data_dir.exists() {
        println!("Nothing to clean — {} does not exist.", data_dir.display());
        return ExitCode::SUCCESS;
    }

    println!("⚠️  WARNING: This will PERMANENTLY DELETE all statefulmemory data at:");
    println!("  {}", data_dir.display());
    println!();
    println!("This includes:");
    println!("  • All observations (decisions, patterns, fixes, feedback, notes)");
    println!("  • All session records");
    println!("  • Daemon socket and logs");
    println!();
    println!("Every memory stored across ALL your projects will be gone.");
    println!("This cannot be undone.");
    println!();

    if !confirm("Type 'yes' to confirm permanent deletion: ") {
        println!("Aborted.");
        return ExitCode::SUCCESS;
    }

    match fs::remove_dir_all(&data_dir) {
        Ok(()) => {
            println!("✓ Deleted {}", data_dir.display());
            println!("All observations wiped. Run `smem obs save` to start fresh.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "statefulmemory clean: failed to remove {}: {e}",
                data_dir.display()
            );
            ExitCode::from(1)
        }
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim().to_lowercase();
    if prompt.contains("Type") {
        answer == "yes"
    } else {
        answer == "y" || answer == "yes"
    }
}

const BLOCK_START: &str = "<!-- statefulmemory-skill-start -->";
const BLOCK_END: &str = "<!-- statefulmemory-skill-end -->";

fn remove_statefulmemory_block(path: &PathBuf) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    if !content.contains(BLOCK_START) {
        return Ok(false);
    }
    let before = content.find(BLOCK_START).unwrap();
    let after = content
        .find(BLOCK_END)
        .map(|i| i + BLOCK_END.len())
        .unwrap_or(content.len());
    let prefix = content[..before].trim_end_matches('\n');
    let suffix = &content[after..];
    let new_content = if suffix.trim().is_empty() {
        format!("{}\n", prefix)
    } else {
        format!("{}\n{}", prefix, suffix)
    };
    fs::write(path, new_content)?;
    Ok(true)
}

fn unpatch_claude_settings(path: &PathBuf) -> std::io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let mut root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let original = root.clone();

    if let Some(arr) = root
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| {
            v.as_str() != Some("Bash(statefulmemory *)") && v.as_str() != Some("Bash(smem *)")
        });
    }

    if let Some(arr) = root
        .pointer_mut("/sandbox/network/allowUnixSockets")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some("~/.statefulmemory/daemon.sock"));
    }

    if let Some(obj) = root.as_object_mut() {
        obj.remove("autoMemoryEnabled");
    }

    if root == original {
        return Ok(false);
    }
    let new_text = serde_json::to_string_pretty(&root).map_err(std::io::Error::other)?;
    fs::write(path, new_text + "\n")?;
    Ok(true)
}

/// Best-effort kill of the install-managed Laya uvicorn process via pidfile.
fn stop_laya_sidecar(home: &Path) {
    let pid_path = home.join(".statefulmemory").join("laya-sidecar.pid");
    let Ok(s) = fs::read_to_string(&pid_path) else {
        return;
    };
    let Ok(pid) = s.trim().parse::<i32>() else {
        let _ = fs::remove_file(&pid_path);
        return;
    };
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
    let _ = fs::remove_file(&pid_path);
}
