// Generated with AI Coding Rules Hub
//! `memlayer uninstall` — remove all skill files installed by `memlayer install`,
//! undo patches to ~/.claude/settings.json, and unregister MCP server entries.
//!
//! `memlayer clean` — stop the daemon and wipe ~/.memlayer/ (all observations).
//!
//! Both commands require explicit confirmation before making any changes.

use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::formatter::Formatter;

pub async fn dispatch_uninstall(_fmt: Formatter) -> ExitCode {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("memlayer uninstall: could not determine home directory");
            return ExitCode::from(1);
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());

    // Enumerate targets so the user sees exactly what will be removed.
    let file_targets: Vec<PathBuf> = vec![
        home.join(".claude").join("skills").join("memlayer").join("SKILL.md"),
        cwd.join(".claude").join("skills").join("memlayer").join("SKILL.md"),
        home.join(".codeium").join("windsurf").join("rules").join("memlayer-memory.md"),
        cwd.join(".windsurf").join("rules").join("memlayer-memory.md"),
        home.join(".cursor").join("rules").join("memlayer.mdc"),
        PathBuf::from("/usr/local/bin/memlayer"),
    ];
    let block_targets: Vec<PathBuf> = vec![
        home.join("CLAUDE.md"),
        home.join(".github").join("copilot-instructions.md"),
    ];
    let settings_path = home.join(".claude").join("settings.json");
    let existing_mcp = crate::mcp_install::existing_registrations(&home, &cwd);

    let existing_files: Vec<&PathBuf> = file_targets.iter().filter(|p| p.exists()).collect();
    let existing_blocks: Vec<&PathBuf> = block_targets.iter().filter(|p| p.exists()).collect();

    if existing_files.is_empty()
        && existing_blocks.is_empty()
        && existing_mcp.is_empty()
        && !settings_path.exists()
    {
        println!("Nothing to uninstall — no memlayer files found.");
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
        match remove_memlayer_block(path) {
            Ok(true)  => removed.push(format!("unblocked  {}", path.display())),
            Ok(false) => {} // block wasn't there
            Err(e)    => failed.push(format!("failed   {} ({e})", path.display())),
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
            Ok(true)  => removed.push(format!("unpatched  {}", settings_path.display())),
            Ok(false) => {}
            Err(e)    => failed.push(format!("failed   {} ({e})", settings_path.display())),
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

pub async fn dispatch_clean(_fmt: Formatter) -> ExitCode {
    let data_dir = memlayer_core::paths::data_dir();

    if !data_dir.exists() {
        println!("Nothing to clean — {} does not exist.", data_dir.display());
        return ExitCode::SUCCESS;
    }

    println!("⚠️  WARNING: This will PERMANENTLY DELETE all memlayer data at:");
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
            println!("All observations wiped. Run `memlayer obs save` to start fresh.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("memlayer clean: failed to remove {}: {e}", data_dir.display());
            ExitCode::from(1)
        }
    }
}

/// Prompt the user and return true only if they answer "y" or "yes" (case-insensitive).
/// For the clean command, require the literal word "yes".
fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim().to_lowercase();
    // For the "Type 'yes'" prompt, require the full word.
    if prompt.contains("Type") {
        answer == "yes"
    } else {
        answer == "y" || answer == "yes"
    }
}

const BLOCK_START: &str = "<!-- memlayer-skill-start -->";
const BLOCK_END: &str = "<!-- memlayer-skill-end -->";

/// Remove the memlayer block from a file that may contain other content.
fn remove_memlayer_block(path: &PathBuf) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    if !content.contains(BLOCK_START) {
        return Ok(false);
    }
    let before = content.find(BLOCK_START).unwrap();
    let after = content.find(BLOCK_END)
        .map(|i| i + BLOCK_END.len())
        .unwrap_or(content.len());
    // Trim the blank line that install_block prepends before the block.
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

/// Undo patches applied by install: remove Bash(memlayer *) from permissions.allow,
/// remove ~/.memlayer/daemon.sock from allowUnixSockets, and re-enable autoMemory.
fn unpatch_claude_settings(path: &PathBuf) -> std::io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let mut root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let original = root.clone();

    // Remove Bash(memlayer *) from permissions.allow
    if let Some(arr) = root.pointer_mut("/permissions/allow").and_then(|v| v.as_array_mut()) {
        arr.retain(|v| v.as_str() != Some("Bash(memlayer *)"));
    }

    // Remove ~/.memlayer/daemon.sock from sandbox.network.allowUnixSockets
    if let Some(arr) = root.pointer_mut("/sandbox/network/allowUnixSockets").and_then(|v| v.as_array_mut()) {
        arr.retain(|v| v.as_str() != Some("~/.memlayer/daemon.sock"));
    }

    // Re-enable autoMemory (remove the key so it reverts to default true)
    if let Some(obj) = root.as_object_mut() {
        obj.remove("autoMemoryEnabled");
    }

    if root == original {
        return Ok(false);
    }
    let new_text = serde_json::to_string_pretty(&root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(path, new_text + "\n")?;
    Ok(true)
}
