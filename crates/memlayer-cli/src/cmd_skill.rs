// Generated with AI Coding Rules Hub
//! `memlayer skill install` — copy the bundled SKILL.md into all detected
//! agent config directories and write agent-specific rule files.
//!
//! Supports:
//!   Claude Code  — ~/.claude/skills/memlayer/SKILL.md  (skill)
//!   Claude Code  — ~/CLAUDE.md  (global instructions, append block)
//!   Windsurf     — ~/.codeium/windsurf/rules/memlayer-memory.md  (global rule)
//!   Cursor       — ~/.cursor/rules/memlayer.mdc  (global rule)
//!   GitHub Copilot — ~/.github/copilot-instructions.md  (append block)
//!
//! Safe to re-run: existing files are overwritten, append targets are
//! idempotent (the memlayer block is replaced, not duplicated).

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::formatter::Formatter;

/// Bundled SKILL.md content compiled into the binary so `skill install`
/// works offline with no network access required.
const SKILL_MD: &str = include_str!("../../../skills/memlayer/SKILL.md");

/// Windsurf / Cursor / Copilot get a condensed rule block — same contract,
/// plain prose rather than the full structured skill format.
const AGENT_RULE: &str = r#"## memlayer memory protocol

memlayer is the persistent memory CLI for this agent. Follow these rules:

MUST run `memlayer obs context --limit 20` at the start of every session
before reading code or making decisions.

MUST run `memlayer obs search "<keyword>"` before introducing a new
pattern, dependency, or convention.

MUST save an observation after:
- A non-obvious architectural or tooling decision.
- A user correction or pushback ("stop doing X").
- A bug fix whose root cause is non-obvious or might recur.

  memlayer obs save \
      --type <decision|pattern|fix|feedback|note> \
      --title "<short, searchable title>" \
      --content "<why — include rejected alternatives and constraints>" \
      --session "$SESSION_ID"

MUST NOT fabricate or cite memories not returned by obs context / obs recent.
MUST NOT save trivial activity (read file, ran tests, edited typo).

Reference: run `memlayer --help` for the full CLI surface.
"#;

/// Marker used to find and replace the memlayer block in append-style files.
const BLOCK_START: &str = "<!-- memlayer-skill-start -->";
const BLOCK_END: &str = "<!-- memlayer-skill-end -->";

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

    // ── Claude Code global ──────────────────────────────────────────────────
    let claude_global = home.join(".claude").join("skills").join("memlayer");
    match install_file(&claude_global.join("SKILL.md"), SKILL_MD) {
        Ok(true)  => installed.push("Claude Code (global)  ~/.claude/skills/memlayer/SKILL.md".into()),
        Ok(false) => skipped.push("Claude Code (global)  (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Claude Code global install failed: {e}"),
    }

    // ── Claude Code local (CWD) ─────────────────────────────────────────────
    // Also install into .claude/skills/ in the current repo so the skill is
    // active even in sessions that predate the global install, or in
    // environments where the global path isn't picked up.
    let claude_local = cwd.join(".claude").join("skills").join("memlayer");
    match install_file(&claude_local.join("SKILL.md"), SKILL_MD) {
        Ok(true)  => installed.push(format!("Claude Code (local)   {}/.claude/skills/memlayer/SKILL.md", cwd.display())),
        Ok(false) => skipped.push("Claude Code (local)   (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Claude Code local install failed: {e}"),
    }

    // ── Windsurf global ─────────────────────────────────────────────────────
    // ~/.codeium/windsurf/rules/*.md — shown in the Windsurf settings GUI.
    let windsurf_global = home.join(".codeium").join("windsurf").join("rules").join("memlayer-memory.md");
    match install_file(&windsurf_global, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Windsurf (global)     {}", windsurf_global.display())),
        Ok(false) => skipped.push("Windsurf (global)     (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Windsurf global install failed: {e}"),
    }

    // ── Windsurf local (CWD) ────────────────────────────────────────────────
    // Per-repo rules in .windsurf/rules/*.md — Windsurf also loads these.
    let windsurf_local = cwd.join(".windsurf").join("rules").join("memlayer-memory.md");
    match install_file(&windsurf_local, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Windsurf (local)      {}", windsurf_local.display())),
        Ok(false) => skipped.push("Windsurf (local)      (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Windsurf local install failed: {e}"),
    }

    // ── Cursor global ───────────────────────────────────────────────────────
    let cursor_path = home.join(".cursor").join("rules").join("memlayer.mdc");
    match install_file(&cursor_path, &format!("---\ndescription: memlayer memory protocol\nalwaysApply: true\n---\n\n{AGENT_RULE}")) {
        Ok(true)  => installed.push(format!("Cursor (global)       {}", cursor_path.display())),
        Ok(false) => skipped.push("Cursor (global)       (unchanged)".into()),
        Err(e)    => eprintln!("  warn: Cursor install failed: {e}"),
    }

    // ── Claude Code global instructions (~/CLAUDE.md) ───────────────────────
    // Append-idempotent block so the memlayer protocol is in Claude Code's
    // user-level instructions even before any skill loads. Preserves any
    // existing content (e.g. AI Coding Rules Hub) by only rewriting the
    // delimited memlayer block.
    let claude_md = home.join("CLAUDE.md");
    match install_block(&claude_md, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Claude Code (CLAUDE.md) {}", claude_md.display())),
        Ok(false) => skipped.push("Claude Code (CLAUDE.md) (unchanged)".into()),
        Err(e)    => eprintln!("  warn: ~/CLAUDE.md install failed: {e}"),
    }

    // ── GitHub Copilot ──────────────────────────────────────────────────────
    let copilot_path = home.join(".github").join("copilot-instructions.md");
    match install_block(&copilot_path, AGENT_RULE) {
        Ok(true)  => installed.push(format!("Copilot (global)      {}", copilot_path.display())),
        Ok(false) => skipped.push("Copilot (global)      (unchanged)".into()),
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

    ExitCode::SUCCESS
}

/// Write `content` to `path`, creating parent dirs as needed.
/// Returns Ok(true) if the file was written, Ok(false) if it was identical.
fn install_file(path: &PathBuf, content: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Skip write if content is already identical.
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
        // Replace the existing memlayer block.
        let before = existing.find(BLOCK_START).unwrap();
        let after = existing.find(BLOCK_END).map(|i| i + BLOCK_END.len()).unwrap_or(existing.len());
        format!("{}{}{}", &existing[..before], block, &existing[after..])
    } else {
        // Append to file.
        if existing.is_empty() {
            block
        } else {
            format!("{}\n\n{}", existing.trim_end(), block)
        }
    };

    if new_content == existing {
        return Ok(false);
    }
    fs::write(path, new_content)?;
    Ok(true)
}
