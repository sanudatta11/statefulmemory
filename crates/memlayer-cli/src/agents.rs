//! Agent detection for `memlayer install`, mirroring skills.sh / `npx skills`.
//!
//! Default install only targets agents whose config directories (or binaries)
//! are present. Override with `--all` or `--agent <id>`.

use std::path::Path;
use std::process::Command;

/// Stable ids accepted by `--agent` (skills.sh-style kebab-case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentId {
    ClaudeCode,
    Cursor,
    Windsurf,
    Antigravity,
    OpenCode,
    KimiCode,
    ZCode,
    /// Shared `.agents/` convention (skills + MCP).
    Agents,
    VsCode,
    CopilotCli,
    /// GitHub Copilot instructions file (`~/.github/copilot-instructions.md`).
    Copilot,
    GeminiCli,
    Codex,
    AmazonQ,
}

impl AgentId {
    pub const ALL: &'static [AgentId] = &[
        Self::ClaudeCode,
        Self::Cursor,
        Self::Windsurf,
        Self::Antigravity,
        Self::OpenCode,
        Self::KimiCode,
        Self::ZCode,
        Self::Agents,
        Self::VsCode,
        Self::CopilotCli,
        Self::Copilot,
        Self::GeminiCli,
        Self::Codex,
        Self::AmazonQ,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Cursor => "cursor",
            Self::Windsurf => "windsurf",
            Self::Antigravity => "antigravity",
            Self::OpenCode => "opencode",
            Self::KimiCode => "kimi-code",
            Self::ZCode => "zcode",
            Self::Agents => "agents",
            Self::VsCode => "vscode",
            Self::CopilotCli => "copilot-cli",
            Self::Copilot => "copilot",
            Self::GeminiCli => "gemini",
            Self::Codex => "codex",
            Self::AmazonQ => "amazon-q",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::Antigravity => "Antigravity",
            Self::OpenCode => "OpenCode",
            Self::KimiCode => "Kimi Code",
            Self::ZCode => "ZCode",
            Self::Agents => "Agents (.agents)",
            Self::VsCode => "VS Code",
            Self::CopilotCli => "Copilot CLI",
            Self::Copilot => "GitHub Copilot",
            Self::GeminiCli => "Gemini CLI",
            Self::Codex => "Codex",
            Self::AmazonQ => "Amazon Q",
        }
    }

    pub fn parse(s: &str) -> Option<AgentId> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "claude-code" | "claude" | "claude_code" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "windsurf" | "codeium" => Some(Self::Windsurf),
            "antigravity" => Some(Self::Antigravity),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "kimi-code" | "kimi" | "kimi_code" => Some(Self::KimiCode),
            "zcode" | "zai" | "z.ai" => Some(Self::ZCode),
            "agents" | "universal" => Some(Self::Agents),
            "vscode" | "vs-code" | "code" => Some(Self::VsCode),
            "copilot-cli" | "github-copilot-cli" => Some(Self::CopilotCli),
            "copilot" | "github-copilot" => Some(Self::Copilot),
            "gemini" | "gemini-cli" => Some(Self::GeminiCli),
            "codex" => Some(Self::Codex),
            "amazon-q" | "amazonq" | "q" => Some(Self::AmazonQ),
            _ => None,
        }
    }

    /// Agents that read the shared `.agents/skills` / `.agents/mcp.json` layout.
    pub fn uses_shared_agents_dir(self) -> bool {
        matches!(
            self,
            Self::Cursor
                | Self::OpenCode
                | Self::KimiCode
                | Self::VsCode
                | Self::Codex
                | Self::GeminiCli
                | Self::CopilotCli
                | Self::Copilot
                | Self::Agents
        )
    }
}

/// How the user chose which agents to install for.
#[derive(Debug, Clone)]
pub struct InstallSelection {
    /// Agents that were present on disk / PATH.
    pub detected: Vec<AgentId>,
    /// Final set to install into (includes auto-expanded `.agents`).
    pub selected: Vec<AgentId>,
    /// Human-readable reason when `selected` is empty.
    pub empty_hint: Option<&'static str>,
}

/// Resolve install targets like skills.sh:
/// - default: detected agents only
/// - `--all`: every known agent
/// - `--agent X`: explicit list (even if not detected)
pub fn resolve_selection(
    home: &Path,
    cwd: &Path,
    all: bool,
    explicit: &[String],
) -> Result<InstallSelection, String> {
    let detected = detect_installed(home, cwd);

    let mut selected = if all {
        AgentId::ALL.to_vec()
    } else if !explicit.is_empty() {
        let mut out = Vec::new();
        for raw in explicit {
            let Some(id) = AgentId::parse(raw) else {
                return Err(format!(
                    "unknown agent `{raw}` — try: {}",
                    AgentId::ALL
                        .iter()
                        .map(|a| a.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            };
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    } else {
        detected.clone()
    };

    // Universal agents share `.agents/` — include it whenever any of them is selected.
    if selected.iter().any(|a| a.uses_shared_agents_dir()) && !selected.contains(&AgentId::Agents)
    {
        selected.push(AgentId::Agents);
    }

    let empty_hint = if selected.is_empty() {
        Some(
            "No coding agents detected. Install an agent, or re-run with \
             `memlayer install --all` or `memlayer install --agent cursor`.",
        )
    } else {
        None
    };

    Ok(InstallSelection {
        detected,
        selected,
        empty_hint,
    })
}

pub fn detect_installed(home: &Path, cwd: &Path) -> Vec<AgentId> {
    AgentId::ALL
        .iter()
        .copied()
        .filter(|a| is_installed(*a, home, cwd))
        .collect()
}

fn is_installed(id: AgentId, home: &Path, cwd: &Path) -> bool {
    match id {
        AgentId::ClaudeCode => {
            home.join(".claude").is_dir()
                || path_on_path("claude")
                || env_truthy("CLAUDECODE")
                || env_truthy("CLAUDE_CODE")
        }
        AgentId::Cursor => {
            home.join(".cursor").is_dir()
                || path_on_path("cursor")
                || env_truthy("CURSOR_TRACE_ID")
        }
        AgentId::Windsurf => home.join(".codeium").join("windsurf").is_dir(),
        AgentId::Antigravity => {
            home.join(".gemini").join("antigravity").is_dir()
                || home.join(".gemini").join("antigravity-cli").is_dir()
                || env_truthy("ANTIGRAVITY_AGENT")
        }
        AgentId::OpenCode => {
            home.join(".config").join("opencode").is_dir()
                || path_on_path("opencode")
                || env_truthy("OPENCODE_CLIENT")
        }
        AgentId::KimiCode => {
            home.join(".kimi-code").is_dir() || home.join(".kimi").is_dir()
        }
        AgentId::ZCode => home.join(".zcode").is_dir(),
        AgentId::Agents => home.join(".agents").is_dir() || cwd.join(".agents").is_dir(),
        AgentId::VsCode => {
            cwd.join(".vscode").is_dir()
                || home.join(".vscode").is_dir()
                || path_on_path("code")
                || path_on_path("code-insiders")
        }
        AgentId::CopilotCli => home.join(".copilot").is_dir() || path_on_path("copilot"),
        AgentId::Copilot => {
            home.join(".github").is_dir()
                || home.join(".copilot").is_dir()
                || env_truthy("COPILOT_MODEL")
        }
        AgentId::GeminiCli => {
            home.join(".gemini").is_dir()
                || path_on_path("gemini")
                || env_truthy("GEMINI_CLI")
        }
        AgentId::Codex => {
            home.join(".codex").is_dir()
                || path_on_path("codex")
                || env_truthy("CODEX_SANDBOX")
        }
        AgentId::AmazonQ => home.join(".aws").join("amazonq").is_dir() || path_on_path("q"),
    }
}

fn env_truthy(key: &str) -> bool {
    // Unit tests assert on directory markers only; ambient agent env vars
    // (e.g. CURSOR_TRACE_ID inside Cursor) would otherwise poison detection.
    if cfg!(test) {
        return false;
    }
    std::env::var_os(key).is_some_and(|v| !v.is_empty())
}

fn path_on_path(bin: &str) -> bool {
    if cfg!(test) {
        return false;
    }
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether `selected` includes this agent (or any of the listed ones).
pub fn selected_any(selected: &[AgentId], agents: &[AgentId]) -> bool {
    agents.iter().any(|a| selected.contains(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_aliases() {
        assert_eq!(AgentId::parse("claude"), Some(AgentId::ClaudeCode));
        assert_eq!(AgentId::parse("kimi"), Some(AgentId::KimiCode));
        assert_eq!(AgentId::parse("nope"), None);
    }

    #[test]
    fn detect_cursor_marker() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let cwd = home.join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        assert!(!detect_installed(home, &cwd).contains(&AgentId::Cursor));
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        assert!(detect_installed(home, &cwd).contains(&AgentId::Cursor));
    }

    #[test]
    fn resolve_expands_agents_for_universal() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let cwd = home.join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let sel = resolve_selection(home, &cwd, false, &["cursor".into()]).unwrap();
        assert!(sel.selected.contains(&AgentId::Cursor));
        assert!(sel.selected.contains(&AgentId::Agents));
    }

    #[test]
    fn resolve_all_includes_everything() {
        let dir = tempdir().unwrap();
        let sel = resolve_selection(dir.path(), dir.path(), true, &[]).unwrap();
        assert_eq!(sel.selected.len(), AgentId::ALL.len());
    }

    #[test]
    fn resolve_empty_when_nothing_detected() {
        let dir = tempdir().unwrap();
        let sel = resolve_selection(dir.path(), dir.path(), false, &[]).unwrap();
        assert!(sel.selected.is_empty());
        assert!(sel.empty_hint.is_some());
    }
}
