//! `memlayer install` — copy the bundled SKILL.md into detected agent
//! config directories, write agent-specific rule files, and register the
//! local stdio MCP server (`memlayer mcp`) with those agents.
//!
//! Auto-detects installed agents (skills.sh-style). Override with
//! `--all` or `--agent <id>`.
//!
//! Supports (skills / rules):
//!   Claude Code, Windsurf, Cursor, GitHub Copilot, shared `.agents/skills`
//!
//! Supports (MCP registration via [`crate::mcp_install`]):
//!   Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code, ZCode,
//!   VS Code / Copilot agent, Copilot CLI, Gemini CLI, Codex, Amazon Q,
//!   shared `.agents/mcp.json`
//!
//! Safe to re-run: existing files are overwritten, append targets are
//! idempotent (the memlayer block is replaced, not duplicated).
//! Settings / MCP patches are additive — existing keys are preserved.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::agents::{self, AgentId};
use crate::cli::InstallArgs;
use crate::formatter::Formatter;

/// Bundled SKILL.md content compiled into the binary so `skill install`
/// works offline with no network access required.
const SKILL_MD: &str = include_str!("../../../skills/memlayer/SKILL.md");

/// Bundled progressive-disclosure reference sidecars. Each tuple is
/// (filename under `references/`, file body). All sidecars install
/// alongside the core SKILL.md via `install_skill_references`.
const SKILL_REFERENCES: &[(&str, &str)] = &[
    (
        "types.md",
        include_str!("../../../skills/memlayer/references/types.md"),
    ),
    (
        "slash.md",
        include_str!("../../../skills/memlayer/references/slash.md"),
    ),
    (
        "hooks.md",
        include_str!("../../../skills/memlayer/references/hooks.md"),
    ),
    (
        "failures.md",
        include_str!("../../../skills/memlayer/references/failures.md"),
    ),
    (
        "examples.md",
        include_str!("../../../skills/memlayer/references/examples.md"),
    ),
];

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
recent observations) — **if the hook is allowed to run**. Look for a
"memlayer briefing" / "Last session summary" / "Pending review" section
in your initial context.

If you see the briefing: do NOT run `obs context` first. Re-run
`memlayer obs context --query "<topic>"` mid-session when switching tasks.

Do NOT rely on the briefing appearing — the SessionStart hook is often
suppressed (managed `allowManagedHooksOnly: true`, or after a `/clear`).
Treat the memory check as keyed off the TASK, not the session: before you
explore code, search files, or propose a fix/implementation for any
substantive task (fix, implement, debug, refactor, "how does X work"), if
no briefing is already in context, run
`memlayer obs context --query "<task>" --limit 20` as step 0 — before any
Grep/Read/exploration. Do this once per task, not on every message.

Before any Grep or Read tool call, memlayer's PreToolUse hooks run
`memlayer hook pre-tool` automatically. If matching prior observations
exist, Grep mode injects them as additional context (markdown to stdout)
and Read mode prints a stderr hint. You do NOT need to manually run
`obs search` before grepping — the hook does it for you. The hook is
silent on misses, capped at 500 ms, and never blocks the tool call.

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
If the memlayer CLI is unavailable (`command not found: memlayer`): the user
likely installed the skill/rule files without the binary. Walk them through
the one-time install — clone, build, symlink, then run `memlayer install`:

  git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer && \
      cd ~/Documents/memlayer && cargo build --release -p memlayer-cli && \
      mkdir -p ~/.local/bin && \
      ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer && \
      ~/.local/bin/memlayer install

Try it yourself first. If your sandbox denies `git clone github.com` or
`cargo` network access (common in managed Claude Code installs), do NOT
keep retrying — ask the user to paste the command into chat with a leading
`!` so Claude Code runs it on their behalf, or to run it in their own
terminal. After they confirm, re-check `command -v memlayer`. If it still
fails, warn the user once and proceed WITHOUT any memory storage.
Do NOT fall back to built-in or auto-memory under any circumstances.
"#;

/// Marker used to find and replace the memlayer block in append-style files.
const BLOCK_START: &str = "<!-- memlayer-skill-start -->";
const BLOCK_END: &str = "<!-- memlayer-skill-end -->";

/// Concise table version for ~/CLAUDE.md — already loaded as instructions,
/// so imperative prose would be redundant. Kept tight to save context tokens.
const CLAUDE_MD_RULE: &str = r#"## memlayer memory protocol

**memlayer is the ONLY memory store. Never use built-in or auto-memory.**

When hooks are allowed to run: SessionStart auto-injects a briefing,
Stop auto-rolls up the session, and PreToolUse[Grep]/PreToolUse[Read]
surface prior observations before grep/read.

**Memory-first, keyed off the task — not the session.** The SessionStart
hook is frequently suppressed (managed `allowManagedHooksOnly: true`, or a
`/clear`), so do NOT rely on a briefing appearing on its own. Before you
explore code, search files, or propose a fix/implementation for ANY
substantive task (fix, implement, debug, refactor, "how does X work"),
FIRST check whether a memlayer briefing ("Last session summary" / "Pending
review") is already in context. If it is, use it. If it is NOT, run
`memlayer obs context --query "<task>" --limit 20` as step 0 of the task —
before any Explore/Grep/Read. Do this once per task, not on every message.

| Trigger | Command |
|---|---|
| Start of any task, no briefing in context | `memlayer obs context --query "<task>" --limit 20` |
| Mid-session task switch | `memlayer obs context --query "<topic>"` |
| Before new pattern/dep/convention | `memlayer obs search "<keyword>"` |
| After decision, fix, or user correction | `memlayer obs save --type <decision\|fix\|feedback\|pattern\|note> --title "..." --content "..." --session "$SESSION_ID"` |
| End of session, if Stop hook is blocked | `memlayer session summarize "$CLAUDE_SESSION_ID" --auto` |
| `/memlayer <text>` or "remember X" | Run `obs save` immediately — no confirmation, no built-in memory |

MUST NOT cite memories not in `obs context` / `obs recent` output.
MUST NOT save trivial activity (read file, ran tests, edited typo).
If CLI unavailable (`command not found: memlayer`): guide the user to
clone + build from https://github.com/sanudatta11/memlayer and run
`memlayer install`. If sandbox blocks the build, ask the user to run it
themselves (a leading `!` in chat works). If still unavailable, warn
once and proceed WITHOUT memory — never fall back to built-in memory.
"#;

/// The bash permission rule added to Claude Code settings.
const CLAUDE_BASH_RULE: &str = "Bash(memlayer *)";

/// The Unix socket path added to Claude Code sandbox settings.
const MEMLAYER_SOCK: &str = "~/.memlayer/daemon.sock";

pub async fn dispatch(_fmt: Formatter, args: InstallArgs) -> ExitCode {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("memlayer install: could not determine home directory");
            return ExitCode::from(1);
        }
    };
    match memlayer_core::config::ensure_config_toml(&home.join(".memlayer")) {
        Ok(r) => {
            if r.created {
                println!(
                    "Wrote ~/.memlayer/config.toml (search.mode={}, extract={}, conflict={}, storage={})",
                    r.search_mode, r.extract, r.conflict, r.backend
                );
            } else if !r.merged_keys.is_empty() {
                println!(
                    "Updated ~/.memlayer/config.toml (filled: {})",
                    r.merged_keys.join(", ")
                );
            } else {
                println!("~/.memlayer/config.toml already complete");
            }
        }
        Err(e) => eprintln!("  warn: could not write ~/.memlayer/config.toml: {e}"),
    }
    if !memlayer_extract::agent_cli::llm_cli_available() {
        eprintln!(
            "  warn: no agent CLI on PATH for extract/judge/rerank \
             (looked for {}). Install your agent's CLI or set MEMLAYER_LLM_BIN.",
            memlayer_extract::agent_cli::known_binaries().join(", ")
        );
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());

    let selection = match agents::resolve_selection(&home, &cwd, args.all, &args.agents) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("memlayer install: {e}");
            return ExitCode::from(1);
        }
    };

    if !selection.detected.is_empty() {
        println!(
            "Detected: {}",
            selection
                .detected
                .iter()
                .map(|a| a.display_name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!("Detected: (none)");
    }

    if let Some(hint) = selection.empty_hint {
        println!();
        println!("{hint}");
        return ExitCode::SUCCESS;
    }

    println!(
        "Installing for: {}",
        selection
            .selected
            .iter()
            .map(|a| a.display_name())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();

    let selected = &selection.selected;
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

    if selected.contains(&AgentId::ClaudeCode) {
        // ── Claude Code global skill ────────────────────────────────────────
        let claude_global = home.join(".claude").join("skills").join("memlayer");
        match install_file(&claude_global.join("SKILL.md"), SKILL_MD) {
            Ok(true)  => installed.push("Claude Code (global)    ~/.claude/skills/memlayer/SKILL.md".into()),
            Ok(false) => skipped.push("Claude Code (global)    (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Claude Code global install failed: {e}"),
        }
        match install_skill_references(&claude_global) {
            Ok(true)  => installed.push("Claude Code (refs)      ~/.claude/skills/memlayer/references/*.md".into()),
            Ok(false) => skipped.push("Claude Code (refs)      (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Claude Code references install failed: {e}"),
        }

        // ── Claude Code local (CWD) ─────────────────────────────────────────
        let claude_local = cwd.join(".claude").join("skills").join("memlayer");
        match install_file(&claude_local.join("SKILL.md"), SKILL_MD) {
            Ok(true)  => installed.push(format!("Claude Code (local)     {}/.claude/skills/memlayer/SKILL.md", cwd.display())),
            Ok(false) => skipped.push("Claude Code (local)     (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Claude Code local install failed: {e}"),
        }
        match install_skill_references(&claude_local) {
            Ok(true)  => installed.push(format!("Claude Code (refs)      {}/.claude/skills/memlayer/references/*.md", cwd.display())),
            Ok(false) => skipped.push("Claude Code (refs local)(unchanged)".into()),
            Err(e)    => eprintln!("  warn: Claude Code local references install failed: {e}"),
        }

        // ── Claude Code settings.json (permissions + socket) ───────────────
        let claude_settings = home.join(".claude").join("settings.json");
        match patch_claude_settings(&claude_settings) {
            Ok(true)  => installed.push("Claude Code (settings)  ~/.claude/settings.json".into()),
            Ok(false) => skipped.push("Claude Code (settings)  (unchanged)".into()),
            Err(e)    => eprintln!("  warn: ~/.claude/settings.json patch failed: {e}"),
        }

        // ── Claude Code project-level settings.json ─────────────────────────
        let project_settings = cwd.join(".claude").join("settings.json");
        if project_settings.exists() {
            match patch_claude_settings_project_hooks(&project_settings) {
                Ok(true)  => installed.push(format!("Claude Code (project)   {}", project_settings.display())),
                Ok(false) => skipped.push("Claude Code (project)   (unchanged)".into()),
                Err(e)    => eprintln!("  warn: {} patch failed: {e}", project_settings.display()),
            }
        }

        // ── Claude Code global instructions (~/CLAUDE.md) ───────────────────
        let claude_md = home.join("CLAUDE.md");
        match install_block(&claude_md, CLAUDE_MD_RULE) {
            Ok(true)  => installed.push(format!("Claude Code (CLAUDE.md) {}", claude_md.display())),
            Ok(false) => skipped.push("Claude Code (CLAUDE.md) (unchanged)".into()),
            Err(e)    => eprintln!("  warn: ~/CLAUDE.md install failed: {e}"),
        }
    }

    if selected.contains(&AgentId::Windsurf) {
        let windsurf_global = home.join(".codeium").join("windsurf").join("rules").join("memlayer-memory.md");
        match install_file(&windsurf_global, AGENT_RULE) {
            Ok(true)  => installed.push(format!("Windsurf (global)       {}", windsurf_global.display())),
            Ok(false) => skipped.push("Windsurf (global)       (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Windsurf global install failed: {e}"),
        }

        let windsurf_local = cwd.join(".windsurf").join("rules").join("memlayer-memory.md");
        match install_file(&windsurf_local, AGENT_RULE) {
            Ok(true)  => installed.push(format!("Windsurf (local)        {}", windsurf_local.display())),
            Ok(false) => skipped.push("Windsurf (local)        (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Windsurf local install failed: {e}"),
        }
    }

    if selected.contains(&AgentId::Cursor) {
        let cursor_path = home.join(".cursor").join("rules").join("memlayer.mdc");
        match install_file(&cursor_path, &format!("---\ndescription: memlayer memory protocol\nalwaysApply: true\n---\n\n{AGENT_RULE}")) {
            Ok(true)  => installed.push(format!("Cursor (global)         {}", cursor_path.display())),
            Ok(false) => skipped.push("Cursor (global)         (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Cursor install failed: {e}"),
        }
    }

    // Shared `.agents/skills` for universal agents (skills.sh canonical path).
    if selected.contains(&AgentId::Agents) {
        for (label, base) in [
            ("Agents (skills user)", home.join(".agents").join("skills").join("memlayer")),
            ("Agents (skills project)", cwd.join(".agents").join("skills").join("memlayer")),
        ] {
            match install_file(&base.join("SKILL.md"), SKILL_MD) {
                Ok(true)  => installed.push(format!("{label:<24} {}", base.join("SKILL.md").display())),
                Ok(false) => skipped.push(format!("{label:<24} (unchanged)")),
                Err(e)    => eprintln!("  warn: {label} install failed: {e}"),
            }
            match install_skill_references(&base) {
                Ok(true)  => installed.push(format!("{label} refs      {}/references/*.md", base.display())),
                Ok(false) => skipped.push(format!("{label} refs      (unchanged)")),
                Err(e)    => eprintln!("  warn: {label} references install failed: {e}"),
            }
        }
    }

    // ── MCP registration (detected / selected agents only) ─────────────────
    for result in crate::mcp_install::install_for(&home, &cwd, selected) {
        match result {
            Ok(action) if action.changed => {
                installed.push(format!(
                    "{:<24} {}",
                    action.label,
                    action.path.display()
                ));
            }
            Ok(action) => {
                skipped.push(format!("{:<24} (unchanged)", action.label));
            }
            Err((label, path, err)) => {
                eprintln!("  warn: {label} MCP patch failed ({}): {err}", path.display());
            }
        }
    }

    if selected.contains(&AgentId::Copilot) || selected.contains(&AgentId::CopilotCli) {
        let copilot_path = home.join(".github").join("copilot-instructions.md");
        match install_block(&copilot_path, AGENT_RULE) {
            Ok(true)  => installed.push(format!("Copilot (global)        {}", copilot_path.display())),
            Ok(false) => skipped.push("Copilot (global)        (unchanged)".into()),
            Err(e)    => eprintln!("  warn: Copilot install failed: {e}"),
        }
    }

    // ── Summary ─────────────────────────────────────────────────────────────
    if installed.is_empty() && skipped.is_empty() {
        println!("Nothing to install for the selected agents.");
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
    println!("        MCP: memory_search / memory_add / memory_context / memory_recent / …");
    println!("        Re-run with --all or --agent <id> to target more agents.");
    println!("        or run `tail -f ~/.memlayer/queries.log` after the new session starts");
    println!("        — you should see an `obs.context` line within a second.");

    // ── Git hooks (re-verify anchors after commit) ───────────────────────────
    let want_hooks = if args.no_git_hooks {
        false
    } else if args.git_hooks {
        true
    } else {
        memlayer_core::git::is_repo(&cwd)
    };
    if want_hooks {
        match crate::git_hooks::install_git_hooks(&cwd) {
            Ok(0) => {
                if memlayer_core::git::is_repo(&cwd) {
                    println!();
                    println!("Git hooks already up-to-date (.git/hooks post-commit/merge/checkout).");
                }
            }
            Ok(n) => {
                println!();
                println!("Installed {n} git hook(s) (post-commit / post-merge / post-checkout).");
                println!("  They run `memlayer verify --quiet` in the background after each commit.");
            }
            Err(e) => eprintln!("  warn: could not install git hooks: {e}"),
        }
    }

    // ── Daemon restart ──────────────────────────────────────────────────────
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
            Ok(_) | Err(_) => {}
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
    ensure_hook(&mut root, "SessionStart", "memlayer hook session-start --limit 20");
    ensure_hook(
        &mut root,
        "Stop",
        "memlayer session summarize \"$CLAUDE_SESSION_ID\" --auto",
    );

    // PreToolUse hooks for Grep + Read — surface relevant prior memlayer
    // observations before the agent runs the search/read. Matcher-keyed
    // (different shape than SessionStart/Stop). Idempotent on (matcher,
    // memlayer-prefixed command).
    ensure_pretool_hook(
        &mut root,
        "Grep",
        "memlayer hook pre-tool --tool Grep --pattern \"$CLAUDE_TOOL_INPUT_pattern\"",
    );
    ensure_pretool_hook(
        &mut root,
        "Read",
        "memlayer hook pre-tool --tool Read --path \"$CLAUDE_TOOL_INPUT_file_path\"",
    );

    if root == original {
        return Ok(false);
    }

    let text = serde_json::to_string_pretty(&root)
        .map_err(std::io::Error::other)?;
    fs::write(path, text + "\n")?;
    Ok(true)
}

/// Patch a project-level `<cwd>/.claude/settings.json` to add memlayer hooks
/// alongside any existing entries (Catalyst, team-shared, etc.). Unlike
/// `patch_claude_settings`, this never touches `permissions`, `sandbox`, or
/// `autoMemoryEnabled` — those belong in the user-level file. Writes a
/// `<path>.bak` before modifying the file so a user can recover if anything
/// goes wrong. Idempotent via the same prefix-match rule as the global patch.
fn patch_claude_settings_project_hooks(path: &Path) -> std::io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let mut root: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    let original = root.clone();

    ensure_hook(&mut root, "SessionStart", "memlayer hook session-start --limit 20");
    ensure_hook(
        &mut root,
        "Stop",
        "memlayer session summarize \"$CLAUDE_SESSION_ID\" --auto",
    );
    ensure_pretool_hook(
        &mut root,
        "Grep",
        "memlayer hook pre-tool --tool Grep --pattern \"$CLAUDE_TOOL_INPUT_pattern\"",
    );
    ensure_pretool_hook(
        &mut root,
        "Read",
        "memlayer hook pre-tool --tool Read --path \"$CLAUDE_TOOL_INPUT_file_path\"",
    );

    if root == original {
        return Ok(false);
    }

    // Backup original before overwriting — project-level settings.json is
    // usually committed to git, so a recoverable copy on-disk is cheap
    // insurance against a botched merge.
    let bak = path.with_extension("json.bak");
    fs::write(&bak, &text)?;

    let new_text = serde_json::to_string_pretty(&root)
        .map_err(std::io::Error::other)?;
    fs::write(path, new_text + "\n")?;
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

/// Ensure a `PreToolUse` hook with a matcher-keyed shape exists. PreToolUse
/// blocks are different from SessionStart/Stop: each block has a `matcher`
/// field naming the tool to fire for. Two memlayer matchers (Grep, Read)
/// coexist, so idempotency is per-`(matcher, memlayer-prefixed command)`,
/// not per-event.
///
/// Schema appended:
/// ```json
/// { "matcher": "<matcher>",
///   "hooks": [{ "type":"command", "command":<cmd>, "timeout":1 }] }
/// ```
/// Timeout is 1s — hooks must never delay the agent's tool call (memlayer's
/// own internal hook command also enforces 500ms).
fn ensure_pretool_hook(root: &mut serde_json::Value, matcher: &str, cmd: &str) {
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

    let pretool_arr = hooks
        .entry("PreToolUse".to_string())
        .or_insert_with(|| Value::Array(vec![]))
        .as_array_mut()
        .expect("PreToolUse entry must be an array");

    // Idempotency: search for an existing block with the same matcher AND
    // any inner hook command starting with "memlayer ". If found, skip.
    let already_present = pretool_arr.iter().any(|block| {
        let block_matcher = block.get("matcher").and_then(|m| m.as_str());
        if block_matcher != Some(matcher) {
            return false;
        }
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

    pretool_arr.push(json!({
        "matcher": matcher,
        "hooks": [
            { "type": "command", "command": cmd, "timeout": 1 }
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

/// Atomically install all SKILL_REFERENCES sidecars under `<skill_dir>/references/`.
/// Each file is written via staged tmp + rename so a crash mid-write never
/// leaves a half-written sidecar in place. Returns Ok(true) if any file was
/// written or modified, Ok(false) if every file was already up to date.
fn install_skill_references(skill_dir: &Path) -> std::io::Result<bool> {
    let refs_dir = skill_dir.join("references");
    fs::create_dir_all(&refs_dir)?;

    let mut any_changed = false;
    for (name, body) in SKILL_REFERENCES {
        let final_path = refs_dir.join(name);
        if final_path.exists() {
            if let Ok(existing) = fs::read_to_string(&final_path) {
                if existing == *body {
                    continue;
                }
            }
        }
        // Stage to a sibling tmp file then atomic rename.
        let tmp_path = refs_dir.join(format!(".{name}.tmp"));
        fs::write(&tmp_path, body)?;
        fs::rename(&tmp_path, &final_path)?;
        any_changed = true;
    }
    Ok(any_changed)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ensure_pretool_hook_inserts_grep_entry() {
        let mut root = json!({});
        ensure_pretool_hook(
            &mut root,
            "Grep",
            "memlayer hook pre-tool --tool Grep --pattern \"$X\"",
        );
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"].as_str(), Some("Grep"));
        let inner = arr[0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert!(inner[0]["command"]
            .as_str()
            .unwrap()
            .starts_with("memlayer hook"));
    }

    #[test]
    fn ensure_pretool_hook_inserts_read_alongside_grep() {
        let mut root = json!({});
        ensure_pretool_hook(&mut root, "Grep", "memlayer hook pre-tool --tool Grep --pattern x");
        ensure_pretool_hook(&mut root, "Read", "memlayer hook pre-tool --tool Read --path y");
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let matchers: Vec<&str> = arr
            .iter()
            .filter_map(|b| b["matcher"].as_str())
            .collect();
        assert!(matchers.contains(&"Grep"));
        assert!(matchers.contains(&"Read"));
    }

    #[test]
    fn ensure_pretool_hook_idempotent_per_matcher() {
        let mut root = json!({});
        ensure_pretool_hook(&mut root, "Grep", "memlayer hook pre-tool --tool Grep --pattern x");
        ensure_pretool_hook(&mut root, "Grep", "memlayer hook pre-tool --tool Grep --pattern x");
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        let grep_blocks: Vec<_> = arr
            .iter()
            .filter(|b| b["matcher"].as_str() == Some("Grep"))
            .collect();
        assert_eq!(
            grep_blocks.len(),
            1,
            "re-invoking ensure_pretool_hook for Grep must NOT duplicate the entry",
        );
    }

    #[test]
    fn ensure_pretool_hook_preserves_user_matchers() {
        let mut root = json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "user-thing" }] }
                ]
            }
        });
        ensure_pretool_hook(&mut root, "Grep", "memlayer hook pre-tool --tool Grep --pattern x");
        let arr = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let bash_intact = arr.iter().any(|b| {
            b["matcher"].as_str() == Some("Bash")
                && b["hooks"][0]["command"].as_str() == Some("user-thing")
        });
        assert!(bash_intact, "user's Bash matcher must be preserved");
    }

    #[test]
    fn project_patch_merges_alongside_catalyst_hooks() {
        // Simulate the exact scenario that bit this repo: project-level
        // settings.json defines Catalyst's SessionStart hook, which shadows
        // memlayer's global one. The patch must append memlayer entries
        // without disturbing Catalyst.
        let tmp = std::env::temp_dir().join(format!(
            "memlayer-test-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let before = json!({
            "hooks": {
                "SessionStart": [
                    {"hooks": [{"type": "command", "command": "catalyst session-start", "timeout": 5}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "catalyst stop", "timeout": 5}]}
                ]
            }
        });
        fs::write(&tmp, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        let changed = patch_claude_settings_project_hooks(&tmp).unwrap();
        assert!(changed, "project file with shadowing hooks must be modified");

        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&tmp).unwrap()).unwrap();
        let ss = after["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 2, "Catalyst entry preserved + memlayer appended");
        assert_eq!(
            ss[0]["hooks"][0]["command"].as_str().unwrap(),
            "catalyst session-start",
            "Catalyst entry must remain first (= runs first)",
        );
        assert!(
            ss[1]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .starts_with("memlayer hook session-start"),
            "memlayer SessionStart appended last",
        );

        // .bak written
        let bak = tmp.with_extension("json.bak");
        assert!(bak.exists(), "backup must be written before overwrite");
        let bak_text = fs::read_to_string(&bak).unwrap();
        let bak_val: serde_json::Value = serde_json::from_str(&bak_text).unwrap();
        assert_eq!(bak_val["hooks"]["SessionStart"].as_array().unwrap().len(), 1,
                   ".bak must contain pre-merge state");

        // Idempotent on re-run
        let changed_again = patch_claude_settings_project_hooks(&tmp).unwrap();
        assert!(!changed_again, "second run must be a no-op");

        let _ = fs::remove_file(&tmp);
        let _ = fs::remove_file(&bak);
    }

    #[test]
    fn project_patch_skips_when_no_project_settings_changes_needed() {
        // If memlayer hooks are already merged, the function should return
        // false and write neither the file nor the .bak.
        let tmp = std::env::temp_dir().join(format!(
            "memlayer-noop-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let already_merged = json!({
            "hooks": {
                "SessionStart": [
                    {"hooks": [{"type": "command", "command": "memlayer hook session-start --limit 20", "timeout": 30}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "memlayer session summarize \"$CLAUDE_SESSION_ID\" --auto", "timeout": 30}]}
                ],
                "PreToolUse": [
                    {"matcher": "Grep", "hooks": [{"type": "command", "command": "memlayer hook pre-tool --tool Grep --pattern x"}]},
                    {"matcher": "Read", "hooks": [{"type": "command", "command": "memlayer hook pre-tool --tool Read --path x"}]}
                ]
            }
        });
        fs::write(&tmp, serde_json::to_string_pretty(&already_merged).unwrap()).unwrap();
        let original_mtime = fs::metadata(&tmp).unwrap().modified().unwrap();

        let changed = patch_claude_settings_project_hooks(&tmp).unwrap();
        assert!(!changed, "already-merged file must be a no-op");
        assert!(!tmp.with_extension("json.bak").exists(), "no .bak on no-op");
        assert_eq!(
            fs::metadata(&tmp).unwrap().modified().unwrap(),
            original_mtime,
            "file must not be rewritten on no-op",
        );

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn ensure_hook_inserts_session_start() {
        let mut root = json!({});
        ensure_hook(&mut root, "SessionStart", "memlayer hook session-start --limit 20");
        let arr = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]["hooks"][0]["command"].as_str().unwrap(),
            "memlayer hook session-start --limit 20",
        );
    }

    #[test]
    fn ensure_hook_idempotent_session_start() {
        let mut root = json!({});
        ensure_hook(&mut root, "SessionStart", "memlayer hook session-start --limit 20");
        ensure_hook(&mut root, "SessionStart", "memlayer hook session-start --limit 20");
        let arr = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "duplicate session-start hook must not be inserted");
    }

    #[test]
    fn ensure_hook_skips_when_old_obs_context_present() {
        // Users who installed before the session-start rename still have
        // `memlayer obs context` — ensure_hook must not add a second block.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    {"hooks": [{"type": "command", "command": "memlayer obs context --limit 20", "timeout": 30}]}
                ]
            }
        });
        ensure_hook(&mut root, "SessionStart", "memlayer hook session-start --limit 20");
        let arr = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "old obs context entry counts as already-present");
    }

    #[test]
    fn ensure_hook_preserves_non_memlayer_stop_hooks() {
        let mut root = json!({
            "hooks": {
                "Stop": [
                    {"hooks": [{"type": "command", "command": "some-other-tool cleanup"}]}
                ]
            }
        });
        ensure_hook(&mut root, "Stop", "memlayer session summarize \"$CLAUDE_SESSION_ID\" --auto");
        let arr = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "non-memlayer Stop hook must be preserved");
        let has_other = arr.iter().any(|b| {
            b["hooks"][0]["command"].as_str() == Some("some-other-tool cleanup")
        });
        assert!(has_other, "original stop hook must still be present");
    }
}
