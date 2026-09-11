# memlayer

<p align="center">
  <img src="website/public/banner.jpg" alt="memlayer" width="100%" />
</p>

Persistent memory for AI coding agents — local, per-project, no cloud.

**Docs:** [https://memlayer.org](https://memlayer.org)

memlayer stores decisions, patterns, fixes, and notes in SQLite and surfaces
the right ones when you (or your agent) start the next session. A thin CLI
talks to a per-user daemon over gRPC; agents can also use MCP tools or
shell hooks.

Works with **Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code,
ZCode, VS Code / Copilot, Codex, Gemini CLI**, and any agent that can shell
out or speak MCP.

[![CI](https://github.com/sanudatta11/memlayer/actions/workflows/ci.yml/badge.svg)](https://github.com/sanudatta11/memlayer/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## What it does

| Primitive | Command / tool | Purpose |
|---|---|---|
| **Save** | `obs save` / `memory_add` | Capture a decision or fix with context |
| **Brief** | `obs context` / `memory_context` | Topic-ranked memory for the current task |
| **Search** | `obs search` / `memory_search` | BM25 (optional hybrid + rerank) |

Data lives under `~/.memlayer/` as plain SQLite you can inspect, back up, or delete.

## Install

**Needs:** Rust stable (≥ 1.75), `protoc`, OpenSSL/`pkg-config`, macOS or Linux (WSL OK).

```bash
git clone https://github.com/sanudatta11/memlayer && cd memlayer
make prereqs && make install    # → ~/.local/bin/memlayer
export PATH="$HOME/.local/bin:$PATH"
memlayer --version
```

The daemon auto-starts on first use. Full guide: [Install](https://memlayer.org/docs/install/).

## Quick start

```bash
memlayer obs save \
  --type decision \
  --title "use pgx not GORM" \
  --content "Team prefers raw SQL via pgx" \
  --session "$(uuidgen)"

memlayer obs recent --limit 5
memlayer obs search "pgx"
```

More: [Getting started](https://memlayer.org/docs/getting-started/).

## Wire into your agent

```bash
memlayer install                 # auto-detect agents (like skills.sh)
memlayer install --agent cursor  # one agent (+ shared .agents)
memlayer install --all           # every known target
```

This installs skills/rules, Claude Code hooks (where applicable), and MCP
registration for detected agents. Restart the agent afterward.

**MCP tools:** `memory_search`, `memory_recent`, `memory_context`,
`memory_add`, `memory_facts`, `memory_health` — launched as `memlayer mcp`.

| Agent | Config touched by install |
|---|---|
| Claude Code | `~/.claude.json`, `.mcp.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Antigravity | `~/.gemini/config/mcp_config.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| Others | Kimi, ZCode, VS Code, Copilot, Codex, Gemini, Amazon Q, `.agents/mcp.json` |

Details: [Wire into your agent](https://memlayer.org/docs/agents/).

## Docs

- [Getting started](https://memlayer.org/docs/getting-started/)
- [Install](https://memlayer.org/docs/install/)
- [Agents / MCP](https://memlayer.org/docs/agents/)
- [Everyday commands](https://memlayer.org/docs/commands/)
- [Config](https://memlayer.org/docs/config/)
- [Troubleshooting](https://memlayer.org/docs/troubleshooting/)

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for layout and local build/test.
Architecture and roadmap: [`CLAUDE.md`](CLAUDE.md), [`docs/ROADMAP.md`](docs/ROADMAP.md).

```bash
cargo build --workspace --tests
cargo test --workspace --lib
```

## License

MIT OR Apache-2.0.
