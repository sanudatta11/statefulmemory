---
title: Wire into your agent
description: Register memlayer with Claude Code, Cursor, OpenCode, Codex, Gemini CLI, and local LLMs via MCP or shell-out.
---

memlayer is persistent memory for AI coding agents: local SQLite under
`~/.memlayer/`, a CLI plus daemon, and MCP tools. Wire it once; agents keep
saving and retrieving decisions across sessions.

```bash
memlayer install                 # auto-detect agents (like skills.sh)
memlayer install --agent cursor  # one agent (+ shared .agents)
memlayer install --agent opencode
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
- `memory_decide`: recommend a decision; auto-runs the resolution judge on open conflicts

## Config files touched by install

| Agent | Config |
|---|---|
| Claude Code | `~/.claude.json`, `.mcp.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Antigravity | `~/.gemini/config/mcp_config.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| Others | Kimi, ZCode, VS Code, Copilot, Codex, Gemini, Amazon Q, `.agents/mcp.json` |

## OpenCode, Codex, and local / open-source models

Repo root [`AGENTS.md`](https://github.com/sanudatta11/memlayer/blob/main/AGENTS.md)
follows the [agents.md](https://agents.md/) convention (OpenCode, Codex, Gemini
CLI, peers). Claude Code / Cursor also use
[`CLAUDE.md`](https://github.com/sanudatta11/memlayer/blob/main/CLAUDE.md).

Pin the CLI and model used for extract, conflict judge, Decide, and optional
rerank (local hybrid search still works without an LLM):

```bash
export MEMLAYER_LLM_BIN=/path/to/opencode   # or cursor-agent, gemini, …
export MEMLAYER_LLM_PROVIDER=opencode
export MEMLAYER_LLM_MODEL=qwen              # or opencode/glm-5.3, …
# Host hints also work: OPENCODE_MODEL, CURSOR_MODEL, GEMINI_MODEL
```

Disable LLM features while keeping BM25 / hybrid retrieval:

```bash
memlayer config set extract.enabled false
memlayer config set conflict.enabled false
```

## Manual fallback (shell-out agents)

```markdown
Before a substantive task, run: memlayer obs context --query "<task>" --limit 20
After a decision or correction, run: memlayer obs save --type decision --title "..." --content "..." --session "$SESSION_ID"
When notes conflict, run: memlayer decide "<question>"
```

## FAQ for agents and LLM assistants

**What is memlayer?**
A local, per-project memory store for coding agents. No cloud required for
core save/search/context.

**How do agents read memory?**
Prefer MCP (`memlayer mcp`) after `memlayer install`. Otherwise shell out to
`memlayer obs context` / `obs search` / `obs save`.

**Where is data stored?**
Under `~/.memlayer/` (per-project SQLite + optional global BM25 mirror).

**How do I keep answers grounded in current code?**
Attach `--anchor path::symbol` when saving; run `memlayer verify`. Context
withdraws stale anchored claims by default (`verify.serve_stale = false`).
