// Generated with AI Coding Rules Hub
//! `memlayer skill install` — copy the bundled SKILL.md into all detected
//! agent config directories and write agent-specific rule files.
//!
//! Supports:
//!   Claude Code  — ~/.claude/skills/memlayer/SKILL.md  (skill)
//!   Claude Code  — ~/CLAUDE.md  (global instructions, append block)
//!   Claude Code  — ~/.claude/settings.json  (permissions + Unix socket)
//!   Windsurf     — ~/.codeium/windsurf/rules/memlayer-memory.md  (global rule)
//!   Cursor       — ~/.cursor/rules/memlayer.mdc  (global rule)
//!   GitHub Copilot — ~/.github/copilot-instructions.md  (append block)
//!
//! Safe to re-run: existing files are overwritten, append targets are
//! idempotent (the memlayer block is replaced, not duplicated).
//! Settings patches are additive — existing keys are preserved.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::formatter::Formatter;

/// Bundled SKILL.md content compiled into the binary so `skill install`
/// works offline with no network access required.
const SKILL_MD: &str = include_str!("../../../skills/memlayer/SKILL.md");

/// Windsurf / Cursor / Copilot / CLAUDE.md get a strict rule block.
/// Same contract as SKILL.md, with explicit no-fallback rule.
const AGENT_RULE: &str = r#"## memlayer memory protocol

IMPORTANT: memlayer is the ONLY memory store for this agent. You MUST NOT use
built-in memory, auto-memory, internal notes, or any other memory mechanism.
All memory reads and writes go through the `memlayer` CLI exclusively.

The binary is at `~/.local/bin/memlayer` (also `/usr/local/bin/memlayer`).
If `memlayer` is not on PATH, use the full path: `~/.local/bin/memlayer`.

At the start of every session memlayer's SessionStart hook automatically
injects the briefing (last session summary + decisions due for review +
recent observations). You do NOT need to run `obs context` first. Re-run
`memlayer obs context --query "<topic>"` mid-session when switching tasks.

Before introducing any new pattern, dependency, library, or naming convention, you MUST run:
  memlayer obs search "<keyword>"

After each of the following events, you MUST save an observation — no exceptions:
- A non-obvious architectural or tooling decision (include rejected alternatives)
- A user correction, pushback, or instruction to stop doing something
- A bug fix whose root cause is non-obvious or might recur
- A discovered convention not already in project documentation

  memlayer obs save \
      --type <decision|pattern|fix|feedback|note> \
      --title "<short, searchable title>" \
      --content "<why — include rejected alternatives and constraints>" \
      --session "$SESSION_ID"

When the user types `/memlayer <text>` or says "remember X" or "save to memlayer":
  Run `obs save` immediately. Do NOT use built-in memory. Do NOT ask for confirmation.

You MUST NOT cite memories that were not returned by `obs context` or `obs recent`.
You MUST NOT save trivial activity such as reading files, running tests, or editing typos.
If the memlayer CLI is unavailable: warn the user once and proceed WITHOUT any memory storage.
Do NOT fall back to built-in or auto-memory under any circumstances.
"#;

/// Marker used to find and replace the memlayer block in append-style files.
const BLOCK_START: &str = "<!-- memlayer-skill-start -->";
const BLOCK_END: &str = "<!-- memlayer-skill-end -->";

/// Concise table version for ~/CLAUDE.md — already loaded as instructions,
/// so imperative prose would be redundant. Kept tight to save context tokens.
const CLAUDE_MD_RULE: &str = r#"## memlayer memory protocol

**memlayer is the ONLY memory store. Never use built-in or auto-memory.**

The SessionStart hook auto-injects a briefing (last session summary +
pending review items + recent observations). The Stop hook auto-rolls up
the session into a summary observation. Don't call `obs context` at
session start manually — it's already done.

| Trigger | Command |
|---|---|
| Mid-session task switch | `memlayer obs context --query "<topic>"` |
| Before new pattern/dep/convention | `memlayer obs search "<keyword>"` |
| After decision, fix, or user correction | `memlayer obs save --type <decision\|fix\|feedback\|pattern\|note> --title "..." --content "..." --session "$SESSION_ID"` |
| `/memlayer <text>` or "remember X" | Run `obs save` immediately — no confirmation, no built-in memory |

MUST NOT cite memories not in `obs context` / `obs recent` output.
MUST NOT save trivial activity (read file, ran tests, edited typo).
If CLI unavailable: warn once, proceed without any memory storage.
"#;

/// The bash permission rule added to Claude Code settings.
const CLAUDE_BASH_RULE: &str = "Bash(memlayer *)";

/// The Unix socket path added to Claude Code sandbox settings.
const MEMLAYER_SOCK: &str = "~/.memlayer/daemon.sock";

pub async fn dispatch(_fmt: Formatter) -> ExitCode {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("memlayer skill install: could not determine home directory");
            return ExitCode::from(1);
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());

    let mut installed: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    // ── /usr/local/bin symlink ──────────────────────────────────────────────
    // GUI apps (Windsurf, Cursor, etc.) don't inherit ~/.zshrc PATH so
    // ~/.local/bin/memlayer is invisible to them. /usr/local/bin is on the
    // default macOS GUI PATH and fixes "command not found" in all agents.
    match install_usr_local_bin_symlink() {
        Ok(true)  => installed.push("/usr/local/bin/memlayer  (GUI PATH symlink)".into()),
        Ok(false) => skipped.push("/usr/local/bin/memlayer  (unchanged)".into()),
        Err(e)    => eprintln!("  warn: /usr/local/bin symlink failed (may need sudo): {e}"),
    }

    // ── Claude Code global skill ────────────────────────────────────────────
    let claude_global = home.join(".claude").join("skills").join("memlayer");
    match install_file(&claude_global.join("SKILL.md"), SKILL_MD) {
        Ok(true)  => installed.push("Claude Code (global)    ~/.claude/skills/memlayer/SKILL.md".into()),
        Ok(false) => skipped.push("Claude Code (global)    (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Claude Code global install failed: {e}"),
    }

    // ── Claude Code local (CWD) ─────────────────────────────────────────────
    let claude_local = cwd.join(".claude").join("skills").join("memlayer");
    match install_file(&claude_local.join("SKILL.md"), SKILL_MD) {
        Ok(true)  => installed.push(format!("Claude Code (local)     {}/.claude/skills/memlayer/SKILL.md", cwd.display())),
        Ok(false) => skipped.push("Claude Code (local)     (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Claude Code local install failed: {e}"),
    }

    // ── Claude Code settings.json (permissions + socket) ───────────────────
    // Adds Bash(memlayer *) to permissions.allow and ~/.memlayer/daemon.sock
    // to sandbox.network.allowUnixSockets so the agent can call memlayer
    // without permission prompts and without the Unix socket being blocked.
    let claude_settings = home.join(".claude").join("settings.json");
    match patch_claude_settings(&claude_settings) {
        Ok(true)  => installed.push("Claude Code (settings)  ~/.claude/settings.json".into()),
        Ok(false) => skipped.push("Claude Code (settings)  (unchanged)".into()),
        Err(e)    => eprintln!("  warn: ~/.claude/settings.json patch failed: {e}"),
    }

    // ── Claude Code global instructions (~/CLAUDE.md) ───────────────────────
    let claude_md = home.join("CLAUDE.md");
    match install_block(&claude_md, CLAUDE_MD_RULE) {
        Ok(true)  => installed.push(format!("Claude Code (CLAUDE.md) {}", claude_md.display())),
        Ok(false) => skipped.push("Claude Code (CLAUDE.md) (unchanged)".into()),
        Err(e)    => eprintln!("  warn: ~/CLAUDE.md install failed: {e}"),
    }

    // ── Windsurf global ─────────────────────────────────────────────────────
    let windsurf_global = home.join(".codeium").join("windsurf").join("rules").join("memlayer-memory.md");
    match install_file(&windsurf_global, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Windsurf (global)       {}", windsurf_global.display())),
        Ok(false) => skipped.push("Windsurf (global)       (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Windsurf global install failed: {e}"),
    }

    // ── Windsurf local (CWD) ────────────────────────────────────────────────
    let windsurf_local = cwd.join(".windsurf").join("rules").join("memlayer-memory.md");
    match install_file(&windsurf_local, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Windsurf (local)        {}", windsurf_local.display())),
        Ok(false) => skipped.push("Windsurf (local)        (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Windsurf local install failed: {e}"),
    }

    // ── Cursor global ───────────────────────────────────────────────────────
    let cursor_path = home.join(".cursor").join("rules").join("memlayer.mdc");
    match install_file(&cursor_path, &format!("---\ndescription: memlayer memory protocol\nalwaysApply: true\n---\n\n{AGENT_RULE}")) {
        Ok(true)  => installed.push(format!("Cursor (global)         {}", cursor_path.display())),
        Ok(false) => skipped.push("Cursor (global)         (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Cursor install failed: {e}"),
    }

    // ── GitHub Copilot ──────────────────────────────────────────────────────
    let copilot_path = home.join(".github").join("copilot-instructions.md");
    match install_block(&copilot_path, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Copilot (global)        {}", copilot_path.display())),
        Ok(false) => skipped.push("Copilot (global)        (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Copilot install failed: {e}"),
    }

    // ── Summary ─────────────────────────────────────────────────────────────
    if installed.is_empty() && skipped.is_empty() {
        println!("No agent targets detected. Nothing installed.");
        println!();
        println!("Tip: install directories are created automatically — re-run");
        println!("after installing an agent to pick it up.");
        return ExitCode::SUCCESS;
    }

    if !installed.is_empty() {
        println!("Installed:");
        for line in &installed {
            println!("  ✓ {line}");
        }
    }
    if !skipped.is_empty() {
        println!("Skipped (already up-to-date):");
        for line in &skipped {
            println!("  - {line}");
        }
    }

    println!();
    println!("Restart your agent for the changes to take effect.");
    println!("Verify: ask the agent \"do you have the memlayer memory protocol?\"");

    // ── Daemon restart ──────────────────────────────────────────────────────
    // After an install (often following a binary upgrade), an old daemon may
    // still be running with the previous binary. Stop it here so the next
    // CLI call auto-spawns the freshly-installed binary, picking up new
    // migrations and behavior changes.
    let socket = memlayer_core::paths::socket_path();
    if socket.exists() {
        match std::process::Command::new(std::env::current_exe().unwrap_or_else(|_| "memlayer".into()))
            .args(["daemon", "stop"])
            .output()
        {
            Ok(out) if out.status.success() => {
                println!();
                println!("Stopped running daemon — next memlayer call will spawn the new binary.");
            }
            Ok(_) | Err(_) => {
                // Daemon wasn't running, or stop failed — both are non-fatal
                // since the auto-spawn path will reconcile on the next call.
            }
        }
    }

    ExitCode::SUCCESS
}

/// Patch ~/.claude/settings.json to allow `memlayer *` bash commands and
/// the daemon Unix socket. Creates the file if it doesn't exist.
/// Returns Ok(true) if the file was modified, Ok(false) if already up-to-date.
fn patch_claude_settings(path: &PathBuf) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut root: serde_json::Value = if path.exists() {
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Object(Default::default()))
    } else {
        serde_json::Value::Object(Default::default())
    };

    let original = root.clone();

    // permissions.allow — ensure "Bash(memlayer *)" is present
    let allow = root
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
        .map(|a| {
            if !a.iter().any(|v| v.as_str() == Some(CLAUDE_BASH_RULE)) {
                a.push(serde_json::Value::String(CLAUDE_BASH_RULE.into()));
            }
        });
    if allow.is_none() {
        // Path doesn't exist yet — build it.
        let obj = root.as_object_mut().unwrap();
        let perms = obj
            .entry("permissions")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let arr = perms
            .as_object_mut()
            .unwrap()
            .entry("allow")
            .or_insert_with(|| serde_json::Value::Array(vec![]));
        let a = arr.as_array_mut().unwrap();
        if !a.iter().any(|v| v.as_str() == Some(CLAUDE_BASH_RULE)) {
            a.push(serde_json::Value::String(CLAUDE_BASH_RULE.into()));
        }
    }

    // sandbox.network.allowUnixSockets — ensure "~/.memlayer/daemon.sock" is present
    let sockets = root
        .pointer_mut("/sandbox/network/allowUnixSockets")
        .and_then(|v| v.as_array_mut())
        .map(|a| {
            if !a.iter().any(|v| v.as_str() == Some(MEMLAYER_SOCK)) {
                a.push(serde_json::Value::String(MEMLAYER_SOCK.into()));
            }
        });
    if sockets.is_none() {
        let obj = root.as_object_mut().unwrap();
        let sandbox = obj
            .entry("sandbox")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let network = sandbox
            .as_object_mut()
            .unwrap()
            .entry("network")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let arr = network
            .as_object_mut()
            .unwrap()
            .entry("allowUnixSockets")
            .or_insert_with(|| serde_json::Value::Array(vec![]));
        let a = arr.as_array_mut().unwrap();
        if !a.iter().any(|v| v.as_str() == Some(MEMLAYER_SOCK)) {
            a.push(serde_json::Value::String(MEMLAYER_SOCK.into()));
        }
    }

    // autoMemoryEnabled: false — disable Claude Code's built-in auto-memory
    // so the agent is forced to use memlayer for all memory operations.
    let obj = root.as_object_mut().unwrap();
    obj.entry("autoMemoryEnabled")
        .or_insert(serde_json::Value::Bool(false));
    // If it was already set to true by the user, override it — memlayer
    // is the declared memory store once this skill is installed.
    if obj.get("autoMemoryEnabled") != Some(&serde_json::Value::Bool(false)) {
        obj.insert("autoMemoryEnabled".into(), serde_json::Value::Bool(false));
    }

    // hooks — auto-inject memlayer context at session start and write a
    // session summary at session end. Idempotent: skips if any hook with a
    // command starting with `memlayer ` already exists for the event.
    ensure_hook(&mut root, "SessionStart", "memlayer obs context --limit 20");
    ensure_hook(
        &mut root,
        "Stop",
        "memlayer session summarize \"$CLAUDE_SESSION_ID\" --auto",
    );

    if root == original {
        return Ok(false);
    }

    let text = serde_json::to_string_pretty(&root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(path, text + "\n")?;
    Ok(true)
}

/// Ensure a Claude Code lifecycle hook is present in `~/.claude/settings.json`.
/// Walks `hooks.<event>[].hooks[]` and appends a new
/// `{type:"command", command:<cmd>, timeout: 30}` entry only if no existing
/// hook for that event has a command beginning with `memlayer ` — preserves
/// user/Catalyst hooks alongside ours and stays idempotent across re-installs.
fn ensure_hook(root: &mut serde_json::Value, event: &str, cmd: &str) {
    use serde_json::{json, Value};

    let obj = match root.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .expect("hooks must be an object");

    let event_arr = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(vec![]))
        .as_array_mut()
        .expect("event entry must be an array");

    // If any existing block contains a command starting with "memlayer ",
    // assume it's our prior install and don't add another.
    let already_present = event_arr.iter().any(|block| {
        block
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.trim_start().starts_with("memlayer "))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });
    if already_present {
        return;
    }

    event_arr.push(json!({
        "hooks": [
            { "type": "command", "command": cmd, "timeout": 30 }
        ]
    }));
}

/// Symlink /usr/local/bin/memlayer → the running binary so GUI apps that
/// don't inherit ~/.zshrc PATH can still find memlayer.
/// Returns Ok(true) written, Ok(false) already correct, Err if no permission.
fn install_usr_local_bin_symlink() -> std::io::Result<bool> {
    let target = std::path::Path::new("/usr/local/bin/memlayer");
    let current_exe = std::env::current_exe()?;

    // Already points at the right place — skip.
    if target.exists() {
        if let Ok(existing) = std::fs::read_link(target) {
            if existing == current_exe {
                return Ok(false);
            }
        } else {
            // Not a symlink (real binary installed there) — leave it alone.
            return Ok(false);
        }
        std::fs::remove_file(target)?;
    }

    if let Some(parent) = target.parent() {
        // /usr/local/bin should already exist; create if somehow missing.
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(&current_exe, target)?;
    Ok(true)
}

/// Write `content` to `path`, creating parent dirs as needed.
/// Returns Ok(true) if the file was written, Ok(false) if it was identical.
fn install_file(path: &PathBuf, content: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let existing = fs::read_to_string(path)?;
        if existing == content {
            return Ok(false);
        }
    }
    fs::write(path, content)?;
    Ok(true)
}

/// Append-or-replace the memlayer block in a file that may have other content.
fn install_block(path: &PathBuf, body: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let block = format!("{BLOCK_START}\n{body}\n{BLOCK_END}\n");

    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let new_content = if existing.contains(BLOCK_START) {
        let before = existing.find(BLOCK_START).unwrap();
        let after = existing.find(BLOCK_END).map(|i| i + BLOCK_END.len()).unwrap_or(existing.len());
        format!("{}{}{}", &existing[..before], block, &existing[after..])
    } else if existing.is_empty() {
        block
    } else {
        format!("{}\n\n{}", existing.trim_end(), block)
    };

    if new_content == existing {
        return Ok(false);
    }
    fs::write(path, new_content)?;
    Ok(true)
}
