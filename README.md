# memlayer

Persistent memory for AI coding agents — local, per-project, no cloud.

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

The daemon auto-starts on first use.

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

## Wire into your agent

```bash
memlayer install                 # auto-detect agents (like skills.sh)
memlayer install --agent cursor  # one agent (+ shared .agents)
memlayer install --all           # every known target
```

This installs skills/rules, Claude Code hooks (where applicable), and MCP
registration for detected agents. Restart the agent afterward.

**MCP tools:** `memory_search`, `memory_recent`, `memory_context`,
`memory_add`, `memory_facts`, `memory_health` — launched as `memlayer mcp`
(stdio; do not run it by hand except for debugging).

| Agent | Config touched by install |
|---|---|
| Claude Code | `~/.claude.json`, `.mcp.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Antigravity | `~/.gemini/config/mcp_config.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| Others | Kimi, ZCode, VS Code, Copilot, Codex, Gemini, Amazon Q, `.agents/mcp.json` |

Manual fallback for any shell-out agent:

```markdown
Before a substantive task, run: memlayer obs context --query "<task>" --limit 20
After a decision or correction, run: memlayer obs save --type decision --title "..." --content "..." --session "$SESSION_ID"
```

## Everyday commands

```bash
memlayer obs save --type decision --title "..." --content "..." --session "$SID"
memlayer obs recent --limit 10
memlayer obs search "auth"                     # BM25
memlayer obs search "auth" --mode hybrid       # + dense (BGE-small + RRF)
memlayer obs context --query "deploy" --limit 20
memlayer obs history <id>                      # supersession chain tree
memlayer obs relations <id>                    # graph relation edges (conflicts_with, supersedes, etc.)
memlayer obs reindex [--force]                 # queue re-embedding and quantization
memlayer tui                                   # interactive observation browser
memlayer doctor [--repair]                     # database integrity audit & auto-repair
memlayer eval --smoke --save-scorecard card.json # retrieval benchmark evaluation
memlayer config show
memlayer daemon status
memlayer logs --lines 50
```

TTY → text; pipes → JSON. Override with `--output {text,json,yaml}`.

## Optional features (off by default)

```bash
memlayer config set extract.enabled true     # LLM fact triples on save
memlayer config set conflict.enabled true    # LLM supersession judge
memlayer config set embed.quantize true      # int8 vectors (~75% smaller)
memlayer obs reindex                         # backfill embeddings
```

Config: `~/.memlayer/config.toml` and per-project overlays. Full knobs and
env overrides: [`CLAUDE.md`](CLAUDE.md).

## Troubleshooting

| Symptom | Fix |
|---|---|
| `command not found` | Put `~/.local/bin` on `PATH` |
| Exit 4 — daemon down | `memlayer daemon force-start` |
| Exit 5 — no project | Run in a git repo, or `export MEMLAYER_PROJECT=…` |
| MCP missing in agent | `memlayer install`, then fully restart the agent |
| Hybrid search empty | `memlayer reindex` |
| Start over | `memlayer daemon stop && rm -rf ~/.memlayer/` |

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for layout and local build/test.
Architecture and roadmap: [`CLAUDE.md`](CLAUDE.md), [`docs/ROADMAP.md`](docs/ROADMAP.md).

```bash
cargo build --workspace --tests
cargo test --workspace --lib
```

## License

MIT OR Apache-2.0.
