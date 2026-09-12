# memlayer

<p align="center">
  <img src="website/public/banner.jpg" alt="memlayer" width="100%" />
</p>

**Persistent memory infrastructure for AI coding agents.**

Thin CLI + MCP → gRPC daemon → per-project SQLite (FTS5 + optional hybrid
BM25/dense). Run **self-hosted** on your machine or team, or use **Memlayer
Cloud** (managed SaaS) when you want hosting done for you.

**Docs:** [https://memlayer.org](https://memlayer.org)

[![CI](https://github.com/sanudatta11/memlayer/actions/workflows/ci.yml/badge.svg)](https://github.com/sanudatta11/memlayer/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## What is Memlayer?

memlayer stores decisions, patterns, fixes, and notes under `~/.memlayer/` as
inspectable SQLite. Agents reach it through the CLI or MCP. The daemon
auto-starts on first use.

## Why do I need it?

Coding agents forget across sessions; chat history is not project memory.
Capture a decision once, retrieve it next session. Optional code anchors let
stale claims withdraw when git moves.

## How is it different?

Memory as **infrastructure for coding agents** — with a real self-host path
(inspectable SQLite, MCP install targets, code anchors) **and** a Cloud SaaS
path for teams that want managed hosting. Not a bare vector index.

Engineering comparison: [Why Memlayer?](https://memlayer.org/docs/why-memlayer/)

## Install

**Needs:** Rust stable (≥ 1.75), `protoc`, OpenSSL/`pkg-config`, macOS or Linux
(WSL OK). Windows is out of scope for v1.

```bash
git clone https://github.com/sanudatta11/memlayer && cd memlayer
make prereqs && make install    # → ~/.local/bin/memlayer
export PATH="$HOME/.local/bin:$PATH"
memlayer --version
```

Full guide: [Install](https://memlayer.org/docs/install/).

## 30-second example

```bash
memlayer install --agent cursor   # or: memlayer install
SID=$(uuidgen)
memlayer obs save --type decision \
  --title "use pgx not GORM" \
  --content "Team prefers raw SQL via pgx" \
  --session "$SID"
memlayer obs context --query "database access layer" --limit 10
memlayer decide "Should we keep pgx?"
```

Agents use MCP (`memlayer mcp`) or shell-out. **Python/TypeScript SDKs are not
shipped.** Preference-evolution walkthrough:
[`demos/preference-evolution.sh`](demos/preference-evolution.sh).

## Architecture

```mermaid
flowchart LR
  Agents[Coding agents MCP or CLI]
  CLI[memlayer CLI]
  MCP[memlayer mcp]
  D[memlayer-daemon gRPC]
  WT[Write thread]
  DB[(project SQLite FTS5 plus optional vec)]
  G[(global.sqlite)]
  Workers[embed extract resolve verify]
  Agents --> CLI
  Agents --> MCP
  CLI --> D
  MCP --> D
  D --> WT
  WT --> DB
  WT --> G
  D --> Workers
  Workers --> DB
```

Details: [Architecture](https://memlayer.org/docs/architecture/).

## Benchmarks

Local LoCoMo / staleness analysis lives in the eval harness. Public claims wait
for disclosed, stratified scorecards — see [LoCoMo eval](https://memlayer.org/docs/locomo/).
Do not quote smoke or tiny slices against published leaderboards.

## Integrations

| Surface | Status |
|---|---|
| MCP: `memory_search`, `memory_recent`, `memory_context`, `memory_add`, `memory_facts`, `memory_health`, `memory_decide` | Shipped (`memlayer mcp`) |
| `memlayer install` — Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code, ZCode, `.agents`, VS Code, Copilot CLI, Copilot, Gemini CLI, Codex, Amazon Q | Shipped |
| LangGraph, OpenAI Agents SDK, CrewAI, AutoGen, LlamaIndex, Vercel AI SDK | **Not yet** |

```bash
memlayer install                 # auto-detect
memlayer install --agent cursor
memlayer install --all
```

Full matrix: [Integrations](https://memlayer.org/docs/integrations/).

## Self-hosting

**Shipped today:** per-user daemon on a Unix domain socket (auto-spawned), or
team mode over TCP+TLS + bearer tokens (`memlayer team init-ca`,
`MEMLAYER_LISTEN=tcp://…`) on infrastructure you run.

Guide: [Self-hosting](https://memlayer.org/docs/self-hosting/).

## Cloud SaaS

**Memlayer Cloud** is the managed SaaS offering for teams that want persistent
agent memory without operating the daemon themselves. Same product thesis
(coding-agent memory, MCP/CLI clients); hosting and ops on us.

Self-host remains first-class and open source. Cloud details and signup will
land on [memlayer.org](https://memlayer.org) as the service rolls out — see
[Self-hosting](https://memlayer.org/docs/self-hosting/#cloud-saas) for how the
modes relate.

## Roadmap

Public directions (graphify bridge, multi-relation judge, eval CI):
[`docs/ROADMAP.md`](docs/ROADMAP.md). Contributor notes:
[`AGENTS.md`](AGENTS.md) / [`CLAUDE.md`](CLAUDE.md).

## Demo

<p align="center">
  <a href="https://memlayer.org/videos/overview-v2.mp4">
    <img src="website/public/videos/overview-v2-poster.jpg" alt="memlayer overview demo" width="100%" />
  </a>
  <br />
  <em>Overview (click to play) — more clips on <a href="https://memlayer.org/">memlayer.org</a></em>
</p>

## Docs

- [Why Memlayer?](https://memlayer.org/docs/why-memlayer/)
- [Architecture](https://memlayer.org/docs/architecture/)
- [Getting started](https://memlayer.org/docs/getting-started/)
- [Install](https://memlayer.org/docs/install/) · [Integrations](https://memlayer.org/docs/integrations/) · [Self-hosting and Cloud](https://memlayer.org/docs/self-hosting/)
- [Observations](https://memlayer.org/docs/observations/) · [Search and context](https://memlayer.org/docs/search-context/)
- [Anchors and verify](https://memlayer.org/docs/anchors-verify/) · [Decide](https://memlayer.org/docs/decide/) · [Mem archives](https://memlayer.org/docs/mem/)
- [Config](https://memlayer.org/docs/config/) · [LoCoMo eval](https://memlayer.org/docs/locomo/) · [Commands](https://memlayer.org/docs/commands/)

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

```bash
cargo build --workspace --tests
cargo test --workspace --lib
```

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or
  https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) or
  https://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in memlayer by you shall be dual-licensed as above, without any
additional terms or conditions.
by you shall be dual-licensed as above, without any
additional terms or conditions.
