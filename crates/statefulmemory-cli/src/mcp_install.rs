//! Multi-agent MCP registration for `statefulmemory install` / `uninstall`.
//!
//! Each coding agent stores MCP config in a slightly different shape. This
//! module knows the durable paths + JSON/TOML schemas and upserts a stdio
//! entry that runs `statefulmemory mcp`.
//!
//! Covered agents (user + project where applicable):
//! - Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code, ZCode
//! - VS Code / Copilot agent, Copilot CLI, Gemini CLI, Codex, Amazon Q
//! - Shared `.agents/mcp.json` convention
//!
//! Install is gated by [`crate::agents`] detection (skills.sh-style) unless
//! the caller passes `--all` / `--agent`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agents::AgentId;

/// Stable server name used in every agent config.
pub const MCP_SERVER_NAME: &str = "statefulmemory";

/// Where a config file is rooted.
#[derive(Debug, Clone, Copy)]
enum Root {
    Home,
    Cwd,
}

/// How to write the statefulmemory stdio server into the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schema {
    /// Top-level `mcpServers.<name> = { type, command, args }`
    McpServers,
    /// Top-level `servers.<name> = { type, command, args }` (VS Code)
    VsCodeServers,
    /// `mcp.<name> = { type: "local", command: [cmd, ...args], enabled }` (OpenCode)
    OpenCodeMcp,
    /// `mcp.servers.<name> = { type: "stdio", command, args }` (ZCode)
    ZcodeMcpServers,
    /// TOML `[mcp_servers.<name>]` with `command` + `args` (Codex)
    CodexToml,
}

struct Target {
    /// Short label shown in install output.
    label: &'static str,
    /// Path relative to home or cwd.
    rel: &'static [&'static str],
    root: Root,
    schema: Schema,
    /// If true, only write when the parent dir (or an agent marker) already exists.
    require_parent: bool,
    /// Install when any of these agents is selected.
    agents: &'static [AgentId],
}

/// All MCP install targets. Order is stable for readable install output.
fn targets() -> &'static [Target] {
    &[
        // ── Claude Code ────────────────────────────────────────────────────
        Target {
            label: "Claude Code (MCP user)",
            rel: &[".claude.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::ClaudeCode],
        },
        Target {
            label: "Claude Code (MCP project)",
            rel: &[".mcp.json"],
            root: Root::Cwd,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::ClaudeCode],
        },
        // ── Cursor ─────────────────────────────────────────────────────────
        Target {
            label: "Cursor (MCP global)",
            rel: &[".cursor", "mcp.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Cursor],
        },
        Target {
            label: "Cursor (MCP project)",
            rel: &[".cursor", "mcp.json"],
            root: Root::Cwd,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Cursor],
        },
        // ── Windsurf ───────────────────────────────────────────────────────
        Target {
            label: "Windsurf (MCP global)",
            rel: &[".codeium", "windsurf", "mcp_config.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Windsurf],
        },
        // ── Antigravity (Gemini) ───────────────────────────────────────────
        Target {
            label: "Antigravity (MCP unified)",
            rel: &[".gemini", "config", "mcp_config.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Antigravity, AgentId::GeminiCli],
        },
        Target {
            label: "Antigravity (MCP legacy)",
            rel: &[".gemini", "antigravity", "mcp_config.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            // Only touch legacy path if that tree already exists.
            require_parent: true,
            agents: &[AgentId::Antigravity],
        },
        Target {
            label: "Antigravity (MCP project)",
            rel: &[".agents", "mcp_config.json"],
            root: Root::Cwd,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Antigravity, AgentId::Agents],
        },
        // ── OpenCode ───────────────────────────────────────────────────────
        Target {
            label: "OpenCode (MCP user)",
            rel: &[".config", "opencode", "opencode.json"],
            root: Root::Home,
            schema: Schema::OpenCodeMcp,
            require_parent: false,
            agents: &[AgentId::OpenCode],
        },
        Target {
            label: "OpenCode (MCP project)",
            rel: &["opencode.json"],
            root: Root::Cwd,
            schema: Schema::OpenCodeMcp,
            require_parent: false,
            agents: &[AgentId::OpenCode],
        },
        // ── Kimi Code ──────────────────────────────────────────────────────
        Target {
            label: "Kimi Code (MCP user)",
            rel: &[".kimi-code", "mcp.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::KimiCode],
        },
        Target {
            label: "Kimi Code (MCP project)",
            rel: &[".kimi-code", "mcp.json"],
            root: Root::Cwd,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::KimiCode],
        },
        // ── ZCode / Z.ai ───────────────────────────────────────────────────
        Target {
            label: "ZCode (MCP user)",
            rel: &[".zcode", "cli", "config.json"],
            root: Root::Home,
            schema: Schema::ZcodeMcpServers,
            require_parent: false,
            agents: &[AgentId::ZCode],
        },
        Target {
            label: "ZCode (MCP project)",
            rel: &[".zcode", "config.json"],
            root: Root::Cwd,
            schema: Schema::ZcodeMcpServers,
            require_parent: false,
            agents: &[AgentId::ZCode],
        },
        // ── Shared .agents convention ──────────────────────────────────────
        Target {
            label: "Agents (MCP user)",
            rel: &[".agents", "mcp.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Agents],
        },
        Target {
            label: "Agents (MCP project)",
            rel: &[".agents", "mcp.json"],
            root: Root::Cwd,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::Agents],
        },
        // ── VS Code / Copilot agent mode ───────────────────────────────────
        Target {
            label: "VS Code (MCP project)",
            rel: &[".vscode", "mcp.json"],
            root: Root::Cwd,
            schema: Schema::VsCodeServers,
            require_parent: false,
            agents: &[AgentId::VsCode],
        },
        // ── Copilot CLI ────────────────────────────────────────────────────
        Target {
            label: "Copilot CLI (MCP)",
            rel: &[".copilot", "mcp-config.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: false,
            agents: &[AgentId::CopilotCli, AgentId::Copilot],
        },
        // ── Gemini CLI ─────────────────────────────────────────────────────
        Target {
            label: "Gemini CLI (MCP)",
            rel: &[".gemini", "settings.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: true, // don't create a bare settings.json for non-users
            agents: &[AgentId::GeminiCli],
        },
        // ── Codex ──────────────────────────────────────────────────────────
        Target {
            label: "Codex (MCP)",
            rel: &[".codex", "config.toml"],
            root: Root::Home,
            schema: Schema::CodexToml,
            require_parent: false,
            agents: &[AgentId::Codex],
        },
        // ── Amazon Q ───────────────────────────────────────────────────────
        Target {
            label: "Amazon Q (MCP)",
            rel: &[".aws", "amazonq", "agents", "default.json"],
            root: Root::Home,
            schema: Schema::McpServers,
            require_parent: true,
            agents: &[AgentId::AmazonQ],
        },
    ]
}

/// Result line for one target after install/uninstall.
pub struct McpAction {
    pub label: String,
    pub path: PathBuf,
    pub changed: bool,
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn ensure_launcher_alias(target: &Path, alias: &Path) -> io::Result<()> {
    if alias == target {
        return Ok(());
    }
    match fs::symlink_metadata(alias) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(alias).ok().as_deref() == Some(target) {
                return Ok(());
            }
            fs::remove_file(alias)?;
        }
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(parent) = alias.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, alias)?;
    #[cfg(not(unix))]
    fs::copy(target, alias)?;
    Ok(())
}

fn install_managed_launcher(source: &Path, target: &Path) -> io::Result<()> {
    if source == target && executable(target) {
        let parent = target.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "managed launcher has no parent")
        })?;
        ensure_launcher_alias(target, &parent.join("smem"))?;
        ensure_launcher_alias(target, &parent.join("sm"))?;
        return Ok(());
    }
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "managed launcher has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let tmp = target.with_extension("launcher.tmp");
    fs::copy(source, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&tmp)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&tmp, permissions)?;
    }
    if target.exists() {
        fs::remove_file(target)?;
    }
    if let Err(e) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if !executable(target) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("managed launcher is not executable: {}", target.display()),
        ));
    }
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "managed launcher has no parent")
    })?;
    ensure_launcher_alias(target, &parent.join("smem"))?;
    ensure_launcher_alias(target, &parent.join("sm"))?;
    Ok(())
}

pub fn ensure_managed_launcher() -> io::Result<PathBuf> {
    let target = statefulmemory_core::paths::managed_statefulmemory_path().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable")
    })?;
    let source = std::env::current_exe()?;
    install_managed_launcher(&source, &target)?;
    Ok(target)
}

pub fn resolve_statefulmemory_command() -> String {
    if let Some(managed) = statefulmemory_core::paths::managed_statefulmemory_path() {
        if executable(&managed) {
            return managed.display().to_string();
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if executable(&exe) {
            return exe.display().to_string();
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    for candidate in [
        home.join(".local/bin/smem"),
        home.join(".local/bin/statefulmemory"),
        PathBuf::from("/opt/homebrew/bin/statefulmemory"),
        PathBuf::from("/opt/homebrew/bin/smem"),
        PathBuf::from("/usr/local/bin/smem"),
        PathBuf::from("/usr/local/bin/statefulmemory"),
    ] {
        if executable(&candidate) {
            return candidate.display().to_string();
        }
    }
    "statefulmemory".to_string()
}

fn join_rel(base: &Path, rel: &[&str]) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in rel {
        p.push(part);
    }
    p
}

fn target_path(t: &Target, home: &Path, cwd: &Path) -> PathBuf {
    let path = match t.root {
        Root::Home => join_rel(home, t.rel),
        Root::Cwd => join_rel(cwd, t.rel),
    };
    if matches!(t.schema, Schema::OpenCodeMcp) {
        let jsonc = path.with_extension("jsonc");
        if jsonc.is_file() {
            return jsonc;
        }
    }
    path
}

fn should_skip(t: &Target, path: &Path) -> bool {
    if !t.require_parent {
        return false;
    }
    match path.parent() {
        Some(parent) => !parent.exists(),
        None => true,
    }
}

fn target_selected(t: &Target, selected: &[AgentId]) -> bool {
    t.agents.iter().any(|a| selected.contains(a))
}

fn stdio_entry(command: &str) -> Value {
    json!({
        "type": "stdio",
        "command": command,
        "args": ["mcp"],
    })
}

fn opencode_entry(command: &str) -> Value {
    json!({
        "type": "local",
        "command": [command, "mcp"],
        "enabled": true,
        "timeout": 30000,
    })
}

fn zcode_entry(command: &str) -> Value {
    json!({
        "type": "stdio",
        "command": command,
        "args": ["mcp"],
    })
}

/// Register statefulmemory MCP with selected agent configs.
pub fn install_for(
    home: &Path,
    cwd: &Path,
    selected: &[AgentId],
) -> Vec<Result<McpAction, (String, PathBuf, String)>> {
    let command = resolve_statefulmemory_command();
    let mut out = Vec::new();
    for t in targets() {
        if !target_selected(t, selected) {
            continue;
        }
        let path = target_path(t, home, cwd);
        if should_skip(t, &path) {
            continue;
        }
        match upsert_target(&path, t.schema, &command) {
            Ok(changed) => out.push(Ok(McpAction {
                label: t.label.to_string(),
                path,
                changed,
            })),
            Err(e) => out.push(Err((t.label.to_string(), path, e.to_string()))),
        }
    }
    out
}

/// Register statefulmemory MCP with every known agent config.
pub fn install_all(home: &Path, cwd: &Path) -> Vec<Result<McpAction, (String, PathBuf, String)>> {
    install_for(home, cwd, AgentId::ALL)
}

/// Remove statefulmemory MCP entries from every known agent config.
pub fn uninstall_all(home: &Path, cwd: &Path) -> Vec<Result<McpAction, (String, PathBuf, String)>> {
    let mut out = Vec::new();
    for t in targets() {
        let path = target_path(t, home, cwd);
        if !path.exists() {
            continue;
        }
        match remove_target(&path, t.schema) {
            Ok(changed) => {
                if changed {
                    out.push(Ok(McpAction {
                        label: t.label.to_string(),
                        path,
                        changed: true,
                    }));
                }
            }
            Err(e) => out.push(Err((t.label.to_string(), path, e.to_string()))),
        }
    }
    out
}

/// Paths that currently contain a statefulmemory MCP entry (for uninstall preview).
pub fn existing_registrations(home: &Path, cwd: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for t in targets() {
        let path = target_path(t, home, cwd);
        if !path.exists() {
            continue;
        }
        if has_statefulmemory_entry(&path, t.schema) {
            out.push((t.label.to_string(), path));
        }
    }
    out
}

fn has_statefulmemory_entry(path: &Path, schema: Schema) -> bool {
    match schema {
        Schema::CodexToml => {
            let Ok(text) = fs::read_to_string(path) else {
                return false;
            };
            let Ok(table) = text.parse::<toml::Table>() else {
                return false;
            };
            table
                .get("mcp_servers")
                .and_then(|v| v.as_table())
                .map(|t| t.contains_key(MCP_SERVER_NAME))
                .unwrap_or(false)
        }
        _ => {
            let Ok(text) = fs::read_to_string(path) else {
                return false;
            };
            let Ok(root) = parse_json_text(&text, path) else {
                return false;
            };
            match schema {
                Schema::McpServers => root
                    .pointer(&format!("/mcpServers/{MCP_SERVER_NAME}"))
                    .is_some(),
                Schema::VsCodeServers => root
                    .pointer(&format!("/servers/{MCP_SERVER_NAME}"))
                    .is_some(),
                Schema::OpenCodeMcp => {
                    root.pointer(&format!("/mcp/{MCP_SERVER_NAME}")).is_some()
                        || root
                            .pointer(&format!("/mcp/servers/{MCP_SERVER_NAME}"))
                            .is_some()
                }
                Schema::ZcodeMcpServers => root
                    .pointer(&format!("/mcp/servers/{MCP_SERVER_NAME}"))
                    .is_some(),
                Schema::CodexToml => false,
            }
        }
    }
}

fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            index += 1;
            continue;
        }
        if c == ',' {
            let mut next = index + 1;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if next < chars.len() && (chars[next] == '}' || chars[next] == ']') {
                index += 1;
                continue;
            }
        }
        out.push(c);
        index += 1;
    }
    out
}

fn parse_json_text(text: &str, path: &Path) -> std::io::Result<Value> {
    let normalized = strip_trailing_commas(&strip_json_comments(text));
    serde_json::from_str(&normalized).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: invalid JSON: {e}", path.display()),
        )
    })
}

fn read_json_object(path: &Path) -> std::io::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    match parse_json_text(&text, path) {
        Ok(Value::Object(map)) => Ok(Value::Object(map)),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: root JSON value must be an object", path.display()),
        )),
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: invalid JSON: {e}", path.display()),
        )),
    }
}

fn write_json(path: &Path, root: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let new_text = serde_json::to_string_pretty(root).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, new_text + "\n")?;
    #[cfg(unix)]
    if let Ok(metadata) = fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() & 0o777);
        fs::set_permissions(&tmp, permissions)?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn ensure_object<'a>(parent: &'a mut Value, key: &str) -> std::io::Result<&'a mut Value> {
    let obj = parent.as_object_mut().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "expected JSON object")
    })?;
    Ok(obj.entry(key).or_insert_with(|| json!({})))
}

fn upsert_map_entry(map: &mut Value, name: &str, entry: Value) -> std::io::Result<bool> {
    let obj = map.as_object_mut().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "expected JSON object map")
    })?;
    let merged = match (obj.get(name), entry.as_object()) {
        (Some(Value::Object(existing)), Some(desired)) => {
            let mut merged = existing.clone();
            for (key, value) in desired {
                if key == "enabled" && merged.contains_key(key) {
                    continue;
                }
                merged.insert(key.clone(), value.clone());
            }
            Value::Object(merged)
        }
        (Some(existing), _) if existing == &entry => existing.clone(),
        _ => entry,
    };
    if obj.get(name) == Some(&merged) {
        Ok(false)
    } else {
        obj.insert(name.to_string(), merged);
        Ok(true)
    }
}

fn upsert_target(path: &Path, schema: Schema, command: &str) -> std::io::Result<bool> {
    match schema {
        Schema::McpServers => {
            let mut root = read_json_object(path)?;
            let original = root.clone();
            let servers = ensure_object(&mut root, "mcpServers")?;
            upsert_map_entry(servers, MCP_SERVER_NAME, stdio_entry(command))?;
            if root == original {
                return Ok(false);
            }
            write_json(path, &root)?;
            Ok(true)
        }
        Schema::VsCodeServers => {
            let mut root = read_json_object(path)?;
            let original = root.clone();
            let servers = ensure_object(&mut root, "servers")?;
            upsert_map_entry(servers, MCP_SERVER_NAME, stdio_entry(command))?;
            if root == original {
                return Ok(false);
            }
            write_json(path, &root)?;
            Ok(true)
        }
        Schema::OpenCodeMcp => {
            let mut root = read_json_object(path)?;
            let original = root.clone();
            let mcp = ensure_object(&mut root, "mcp")?;
            // Prefer nested mcp.servers when that layout already exists (v2).
            let entry = opencode_entry(command);
            if mcp.get("servers").and_then(|v| v.as_object()).is_some() {
                let servers = ensure_object(mcp, "servers")?;
                upsert_map_entry(servers, MCP_SERVER_NAME, entry)?;
            } else {
                upsert_map_entry(mcp, MCP_SERVER_NAME, entry)?;
            }
            if root == original {
                return Ok(false);
            }
            write_json(path, &root)?;
            Ok(true)
        }
        Schema::ZcodeMcpServers => {
            let mut root = read_json_object(path)?;
            let original = root.clone();
            let mcp = ensure_object(&mut root, "mcp")?;
            let servers = ensure_object(mcp, "servers")?;
            upsert_map_entry(servers, MCP_SERVER_NAME, zcode_entry(command))?;
            if root == original {
                return Ok(false);
            }
            write_json(path, &root)?;
            Ok(true)
        }
        Schema::CodexToml => upsert_codex_toml(path, command),
    }
}

fn write_toml(path: &Path, table: &toml::Table) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, format!("{}\n", table))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn upsert_codex_toml(path: &Path, command: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut table: toml::Table = if path.exists() {
        let text = fs::read_to_string(path)?;
        if text.trim().is_empty() {
            toml::Table::new()
        } else {
            text.parse().map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: invalid TOML: {e}", path.display()),
                )
            })?
        }
    } else {
        toml::Table::new()
    };

    let servers = table
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let servers_tbl = servers.as_table_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mcp_servers must be a TOML table",
        )
    })?;

    let mut entry = toml::Table::new();
    entry.insert("command".into(), toml::Value::String(command.into()));
    entry.insert(
        "args".into(),
        toml::Value::Array(vec![toml::Value::String("mcp".into())]),
    );
    let entry_val = match servers_tbl.get(MCP_SERVER_NAME) {
        Some(toml::Value::Table(existing)) => {
            let mut merged = existing.clone();
            merged.insert("command".into(), toml::Value::String(command.into()));
            merged.insert(
                "args".into(),
                toml::Value::Array(vec![toml::Value::String("mcp".into())]),
            );
            toml::Value::Table(merged)
        }
        Some(existing) if existing == &toml::Value::Table(entry.clone()) => {
            toml::Value::Table(entry)
        }
        _ => toml::Value::Table(entry),
    };

    let changed = servers_tbl.get(MCP_SERVER_NAME) != Some(&entry_val);
    if !changed {
        return Ok(false);
    }
    servers_tbl.insert(MCP_SERVER_NAME.into(), entry_val);
    write_toml(path, &table)?;
    Ok(true)
}

fn remove_target(path: &Path, schema: Schema) -> std::io::Result<bool> {
    match schema {
        Schema::CodexToml => remove_codex_toml(path),
        Schema::McpServers => remove_json_map_entry(path, &["mcpServers"], true),
        Schema::VsCodeServers => remove_json_map_entry(path, &["servers"], true),
        Schema::OpenCodeMcp => {
            // Try nested mcp.servers first, then flat mcp.<name>.
            let nested = remove_json_map_entry(path, &["mcp", "servers"], false)?;
            let flat = remove_json_map_entry(path, &["mcp"], true)?;
            Ok(nested || flat)
        }
        Schema::ZcodeMcpServers => remove_json_map_entry(path, &["mcp", "servers"], true),
    }
}

fn remove_json_map_entry(
    path: &Path,
    map_path: &[&str],
    delete_empty_dedicated: bool,
) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut root = read_json_object(path)?;
    let original = root.clone();

    // Walk to the parent of the leaf map, then remove MCP_SERVER_NAME.
    let mut cursor = &mut root;
    for (i, key) in map_path.iter().enumerate() {
        let is_last = i + 1 == map_path.len();
        if is_last {
            let Some(obj) = cursor.as_object_mut() else {
                return Ok(false);
            };
            let Some(map) = obj.get_mut(*key).and_then(|v| v.as_object_mut()) else {
                return Ok(false);
            };
            if map.remove(MCP_SERVER_NAME).is_none() {
                return Ok(false);
            }
            if map.is_empty() {
                obj.remove(*key);
            }
        } else {
            let Some(next) = cursor.as_object_mut().and_then(|o| o.get_mut(*key)) else {
                return Ok(false);
            };
            cursor = next;
        }
    }

    // Prune empty nested objects along the path (e.g. empty `mcp: {}`).
    prune_empty_objects(&mut root, map_path);

    if root == original {
        return Ok(false);
    }

    let is_dedicated = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
        n == "mcp.json" || n == "mcp_config.json" || n == "mcp-config.json" || n == ".mcp.json"
    });
    if delete_empty_dedicated && is_dedicated && root.as_object().is_some_and(|o| o.is_empty()) {
        fs::remove_file(path)?;
        return Ok(true);
    }

    write_json(path, &root)?;
    Ok(true)
}

fn prune_empty_objects(root: &mut Value, map_path: &[&str]) {
    // Only prune intermediate keys when the leaf was removed and the container
    // is now empty. Walk from the outside: if `mcp` only held `servers` and
    // servers is gone, drop `mcp`.
    if map_path.len() < 2 {
        return;
    }
    let parent_keys = &map_path[..map_path.len() - 1];
    let mut cursor = root;
    for (i, key) in parent_keys.iter().enumerate() {
        let is_last = i + 1 == parent_keys.len();
        let Some(obj) = cursor.as_object_mut() else {
            return;
        };
        if is_last {
            if let Some(child) = obj.get(*key) {
                if child.as_object().is_some_and(|o| o.is_empty()) {
                    obj.remove(*key);
                }
            }
            return;
        }
        match obj.get_mut(*key) {
            Some(next) => cursor = next,
            None => return,
        }
    }
}

fn remove_codex_toml(path: &Path) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)?;
    let mut table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let Some(servers) = table.get_mut("mcp_servers").and_then(|v| v.as_table_mut()) else {
        return Ok(false);
    };
    if servers.remove(MCP_SERVER_NAME).is_none() {
        return Ok(false);
    }
    if servers.is_empty() {
        table.remove("mcp_servers");
    }
    if table.is_empty() {
        // Don't delete Codex's whole config.toml — leave an empty-ish file only
        // if it had nothing else; still write remaining content.
    }
    write_toml(path, &table)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn mcp_servers_upsert_and_remove() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        assert!(upsert_target(&path, Schema::McpServers, "/bin/statefulmemory").unwrap());
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["statefulmemory"]["args"][0], "mcp");
        assert!(!upsert_target(&path, Schema::McpServers, "/bin/statefulmemory").unwrap());
        assert!(remove_target(&path, Schema::McpServers).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn opencode_jsonc_is_accepted_and_preserves_other_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.jsonc");
        fs::write(
            &path,
            "{\n  // keep this setting\n  \"theme\": \"dark\",\n  \"mcp\": {\"other\": {}},\n}\n",
        )
        .unwrap();
        assert!(upsert_target(&path, Schema::OpenCodeMcp, "statefulmemory").unwrap());
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcp"]["statefulmemory"]["command"][1], "mcp");
    }

    #[test]
    fn malformed_json_is_not_replaced() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let original = "{ this is not valid json";
        fs::write(&path, original).unwrap();
        assert!(upsert_target(&path, Schema::McpServers, "statefulmemory").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn opencode_flat_and_nested() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.json");
        assert!(upsert_target(&path, Schema::OpenCodeMcp, "statefulmemory").unwrap());
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcp"]["statefulmemory"]["type"], "local");
        assert_eq!(v["mcp"]["statefulmemory"]["command"][1], "mcp");

        // Nested layout preference when servers already exists.
        let path2 = dir.path().join("opencode2.json");
        fs::write(
            &path2,
            r#"{"mcp":{"servers":{"other":{"type":"local","command":["x"]}}}}"#,
        )
        .unwrap();
        assert!(upsert_target(&path2, Schema::OpenCodeMcp, "statefulmemory").unwrap());
        let v2: Value = serde_json::from_str(&fs::read_to_string(&path2).unwrap()).unwrap();
        assert!(v2["mcp"]["servers"]["statefulmemory"].is_object());
        assert!(v2["mcp"]["servers"]["other"].is_object());
    }

    #[test]
    fn zcode_nested_servers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert!(upsert_target(&path, Schema::ZcodeMcpServers, "statefulmemory").unwrap());
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcp"]["servers"]["statefulmemory"]["type"], "stdio");
    }

    #[test]
    fn vscode_servers_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        assert!(upsert_target(&path, Schema::VsCodeServers, "statefulmemory").unwrap());
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["servers"]["statefulmemory"].is_object());
        assert!(v.get("mcpServers").is_none());
    }

    #[test]
    fn codex_toml_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(upsert_target(&path, Schema::CodexToml, "/usr/local/bin/statefulmemory").unwrap());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[mcp_servers.statefulmemory]"));
        assert!(text.contains("command = \"/usr/local/bin/statefulmemory\""));
        assert!(remove_target(&path, Schema::CodexToml).unwrap());
        let after = fs::read_to_string(&path).unwrap();
        assert!(!after.contains("statefulmemory"));
    }

    #[test]
    fn managed_launcher_is_atomic_and_executable() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("bin").join("statefulmemory");
        fs::write(&source, b"binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&source).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&source, permissions).unwrap();
        }
        install_managed_launcher(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"binary");
        assert!(executable(&target));
        #[cfg(unix)]
        {
            let smem = dir.path().join("bin").join("smem");
            let sm = dir.path().join("bin").join("sm");
            assert_eq!(fs::read_link(&smem).unwrap(), target);
            assert_eq!(fs::read_link(&sm).unwrap(), target);
        }
        fs::write(&source, b"new-binary").unwrap();
        install_managed_launcher(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new-binary");
    }

    #[test]
    fn install_all_writes_core_targets() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let results = install_all(home.path(), cwd.path());
        let changed: Vec<_> = results
            .into_iter()
            .filter_map(|r| r.ok())
            .filter(|a| a.changed)
            .collect();
        assert!(
            changed.iter().any(|a| a.label.contains("Claude Code")),
            "expected Claude Code target"
        );
        assert!(
            changed.iter().any(|a| a.label.contains("Cursor")),
            "expected Cursor target"
        );
        assert!(
            changed.iter().any(|a| a.label.contains("Antigravity")),
            "expected Antigravity target"
        );
        assert!(
            changed.iter().any(|a| a.label.contains("OpenCode")),
            "expected OpenCode target"
        );
        assert!(
            changed.iter().any(|a| a.label.contains("Kimi")),
            "expected Kimi target"
        );
        assert!(
            changed.iter().any(|a| a.label.contains("ZCode")),
            "expected ZCode target"
        );
        assert!(
            changed.iter().any(|a| a.label.contains("Windsurf")),
            "expected Windsurf target"
        );
    }

    #[test]
    fn require_parent_skips_missing_trees() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        // Amazon Q requires parent; without ~/.aws/amazonq/agents it is skipped.
        let results = install_all(home.path(), cwd.path());
        assert!(!results.iter().any(|r| {
            r.as_ref()
                .ok()
                .is_some_and(|a| a.label.contains("Amazon Q"))
        }));
    }
}
