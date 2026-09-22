---
title: Wire into your agent
description: Register statefulmemory with Claude Code, Cursor, OpenCode, Codex, Gemini CLI, and local LLMs via MCP or shell-out.
---

statefulmemory is persistent memory for AI coding agents: local SQLite under
`~/.statefulmemory/`, a CLI plus daemon, and MCP tools. Wire it once; agents keep
saving and retrieving decisions across sessions.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/install-agents-v2.mp4" title="Install and agents demo">
  Your browser does not support video.
</video>

## Install targets

```bash
statefulmemory install                 # auto-detect agents (like skills.sh)
statefulmemory install --agent cursor  # one agent (+ shared .agents)
statefulmemory install --agent opencode
statefulmemory install --all           # every known target
statefulmemory install --no-git-hooks  # skip verify hooks
```

This installs skills/rules, Claude Code hooks (where applicable), and MCP
registration for detected agents. When cwd is a git repo, it also installs
`post-commit` / `post-merge` / `post-checkout` hooks that run
`statefulmemory verify --quiet` (skip with `--no-git-hooks`). Restart the agent
afterward.

## MCP tools

Launched as `statefulmemory mcp` (stdio; do not run it by hand except for debugging):

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

Repo root [`AGENTS.md`](https://github.com/sanudatta11/statefulmemory/blob/main/AGENTS.md)
follows the [agents.md](https://agents.md/) convention (OpenCode, Codex, Gemini
CLI, peers). Claude Code / Cursor also use
[`CLAUDE.md`](https://github.com/sanudatta11/statefulmemory/blob/main/CLAUDE.md).

Pin the CLI and model used for extract, conflict judge, Decide, and optional
rerank (local hybrid search still works without an LLM):

```bash
export STATEFULMEMORY_LLM_BIN=/path/to/opencode   # or cursor-agent, gemini, …
export STATEFULMEMORY_LLM_PROVIDER=opencode
export STATEFULMEMORY_LLM_MODEL=qwen              # or opencode/glm-5.3, …
# Host hints also work: OPENCODE_MODEL, CURSOR_MODEL, GEMINI_MODEL
```

Disable LLM features while keeping BM25 / hybrid retrieval:

```bash
statefulmemory config set extract.enabled false
statefulmemory config set conflict.enabled false
```

## Manual fallback (shell-out agents)

```markdown
Before a substantive task, run: statefulmemory obs context --query "<task>" --limit 20
After a decision or correction, run: statefulmemory obs save --type decision --title "..." --content "..." --session "$SESSION_ID"
When notes conflict, run: statefulmemory decide "<question>"
```

## FAQ for agents and LLM assistants

**What is statefulmemory?**
Persistent memory for coding agents. Self-host (local or team TCP) or
StatefulMemory Cloud (managed SaaS). Agents use MCP or the CLI.

**How do agents read memory?**
Prefer MCP (`statefulmemory mcp`) after `statefulmemory install`. Otherwise shell out to
`statefulmemory obs context` / `obs search` / `obs save`.

**Where is data stored?**
Under `~/.statefulmemory/` (per-project SQLite + optional global BM25 mirror).

**How do I keep answers grounded in current code?**
Attach `--anchor path::symbol` when saving; run `statefulmemory verify`. Context
withdraws stale anchored claims by default (`verify.serve_stale = false`).
