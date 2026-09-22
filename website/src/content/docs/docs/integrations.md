---
title: Integrations
description: Shipped statefulmemory install targets and MCP tools versus wishlist frameworks that are not available yet.
---

StatefulMemory wires into coding agents via **`statefulmemory install`** (skills / rules /
MCP registration) and the stdio MCP server **`statefulmemory mcp`**. Agents can also
shell out to the CLI. There is no Python or TypeScript Memory SDK today.

## Shipped: install targets (14)

```bash
statefulmemory install                 # auto-detect
statefulmemory install --agent cursor  # one agent (+ shared .agents when needed)
statefulmemory install --all
```

| Id | Agent |
|---|---|
| `claude-code` | Claude Code |
| `cursor` | Cursor |
| `windsurf` | Windsurf |
| `antigravity` | Antigravity |
| `opencode` | OpenCode |
| `kimi-code` | Kimi Code |
| `zcode` | ZCode |
| `agents` | Shared `.agents/` |
| `vscode` | VS Code |
| `copilot-cli` | Copilot CLI |
| `copilot` | GitHub Copilot |
| `gemini` | Gemini CLI |
| `codex` | Codex |
| `amazon-q` | Amazon Q |

Config paths and local-LLM env: [Wire into your agent](/docs/agents/).

## Shipped: MCP tools

Launched as `statefulmemory mcp` (stdio):

| Tool | Purpose |
|---|---|
| `memory_search` | Hybrid / BM25 search |
| `memory_recent` | Recent observations |
| `memory_context` | Topic-ranked brief for the current task |
| `memory_add` | Save an observation |
| `memory_facts` | Atomic facts for an observation |
| `memory_health` | Daemon / project health |
| `memory_decide` | Recommend a decision; resolve open conflicts |

## Near-term

Deepen Cursor / Claude Code / OpenCode installs. Optional language clients may
appear later; they are **not** shipped.

## Not yet (wishlist)

These are **not available** as first-party integrations today — do not treat
them as shipped:

- LangGraph
- OpenAI Agents SDK
- CrewAI
- AutoGen
- LlamaIndex
- Vercel AI SDK
- Python / TypeScript Memory SDK

**StatefulMemory Cloud** (managed SaaS) is a product offering alongside self-host —
see [Self-hosting and Cloud](/docs/self-hosting/). It is not an “integration”
in the agent-framework sense.

See [Why statefulmemory](/docs/why-statefulmemory/) for positioning.
