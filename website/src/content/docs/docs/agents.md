---
title: Wire into your agent
description: Register memlayer with Claude Code, Cursor, Windsurf, and other coding agents.
---

```bash
memlayer install                 # auto-detect agents (like skills.sh)
memlayer install --agent cursor  # one agent (+ shared .agents)
memlayer install --all           # every known target
```

This installs skills/rules, Claude Code hooks (where applicable), and MCP
registration for detected agents. Restart the agent afterward.

## MCP tools

Launched as `memlayer mcp` (stdio; do not run it by hand except for debugging):

- `memory_search`
- `memory_recent`
- `memory_context`
- `memory_add`
- `memory_facts`
- `memory_health`

## Config files touched by install

| Agent | Config |
|---|---|
| Claude Code | `~/.claude.json`, `.mcp.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Antigravity | `~/.gemini/config/mcp_config.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| Others | Kimi, ZCode, VS Code, Copilot, Codex, Gemini, Amazon Q, `.agents/mcp.json` |

## Manual fallback (shell-out agents)

```markdown
Before a substantive task, run: memlayer obs context --query "<task>" --limit 20
After a decision or correction, run: memlayer obs save --type decision --title "..." --content "..." --session "$SESSION_ID"
```
