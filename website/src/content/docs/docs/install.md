---
title: Install
description: Build and install the statefulmemory CLI on macOS or Linux.
---

**Needs:** Rust stable (≥ 1.75), `protoc`, OpenSSL / `pkg-config`, macOS or Linux (WSL OK).
**Python 3.10+** is recommended so `install` can set up the Laya System-1 sidecar
out of the box (typed decide / conflict / query router).

## Build from source

```bash
git clone https://github.com/sanudatta11/statefulmemory && cd statefulmemory
make prereqs && make install    # → ~/.local/bin/statefulmemory (+ smem, sm)
export PATH="$HOME/.local/bin:$PATH"
statefulmemory --version        # same binary: smem --version / sm --version
```

The daemon auto-starts on first use (self-host default: Unix domain socket).
Put `~/.local/bin` on your `PATH` permanently if `statefulmemory` / `smem` / `sm` is not
found after install. Team TCP self-host and **StatefulMemory Cloud** (managed SaaS): see
[Self-hosting and Cloud](/docs/self-hosting/).

## What install puts where

| Path | Role |
| --- | --- |
| `~/.local/bin/statefulmemory` (+ `smem`, `sm`) | CLI binary (same code) |
| `~/.statefulmemory/` | Data dir (created on first use) |
| `~/.statefulmemory/config.toml` | Written/merged by `install` (includes `[laya]` when missing) |
| `~/.statefulmemory/laya-sidecar/` | Laya FastAPI app (unless `--no-laya`) |
| `~/.statefulmemory/laya-venv/` | Dedicated Python venv for Laya |

## Wire agents, git hooks, and Laya

From any project directory (after the binary is on `PATH`):

```bash
statefulmemory install                 # or: smem install / sm install
statefulmemory install --no-git-hooks  # skip post-commit / post-merge / post-checkout
statefulmemory install --no-laya       # skip Python venv / sidecar spawn
```

When cwd is a git repo, install adds hooks that run `statefulmemory verify --quiet`
after commit, merge, and checkout. That keeps code-anchored memories marked
when HEAD moves. Uninstall strips the statefulmemory blocks from those hooks.

`install` also writes a bootstrap `~/.statefulmemory/config.toml` (hybrid search,
conflict judge, extract, **Laya**) without clobbering keys you already set.

Laya details: [repo `tools/laya-sidecar/README.md`](https://github.com/sanudatta11/statefulmemory/blob/main/tools/laya-sidecar/README.md).

Full agent matrix: [Wire into your agent](/docs/agents/).

## Sanity checks

```bash
statefulmemory daemon status        # same as: smem daemon status / sm daemon status
statefulmemory doctor              # includes Laya enabled / url / health
```

## Next steps

1. [Getting started](/docs/getting-started/): end-to-end tour
2. [Wire into your agent](/docs/agents/): MCP tools and hooks
3. [Config](/docs/config/): `[laya]` and other knobs
4. [GitHub](https://github.com/sanudatta11/statefulmemory): source, issues, and releases
