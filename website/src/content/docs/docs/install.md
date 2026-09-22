---
title: Install
description: Build and install the statefulmemory CLI on macOS or Linux.
---

**Needs:** Rust stable (≥ 1.75), `protoc`, OpenSSL / `pkg-config`, macOS or Linux (WSL OK).

## Build from source

```bash
git clone https://github.com/sanudatta11/statefulmemory && cd statefulmemory
make prereqs && make install    # → ~/.local/bin/statefulmemory
export PATH="$HOME/.local/bin:$PATH"
statefulmemory --version
```

The daemon auto-starts on first use (self-host default: Unix domain socket).
Put `~/.local/bin` on your `PATH` permanently if `statefulmemory` is not found after
install. Team TCP self-host and **StatefulMemory Cloud** (managed SaaS): see
[Self-hosting and Cloud](/docs/self-hosting/).

## What install puts where

| Path | Role |
| --- | --- |
| `~/.local/bin/statefulmemory` | CLI binary |
| `~/.statefulmemory/` | Data dir (created on first use) |
| `~/.statefulmemory/config.toml` | Written/merged by `statefulmemory install` |

## Wire agents and git hooks

From any project directory (after the binary is on `PATH`):

```bash
statefulmemory install                 # MCP + skills; git hooks if cwd is a repo
statefulmemory install --no-git-hooks  # skip post-commit / post-merge / post-checkout
```

When cwd is a git repo, install adds hooks that run `statefulmemory verify --quiet`
after commit, merge, and checkout. That keeps code-anchored memories marked
when HEAD moves. Uninstall strips the statefulmemory blocks from those hooks.

`statefulmemory install` also writes a bootstrap `~/.statefulmemory/config.toml` (hybrid
search, conflict judge, extract on) without clobbering keys you already set.

Full agent matrix: [Wire into your agent](/docs/agents/).

## Sanity checks

```bash
statefulmemory daemon status
statefulmemory doctor
```

## Next steps

1. [Getting started](/docs/getting-started/): end-to-end tour
2. [Wire into your agent](/docs/agents/): MCP tools and hooks
3. [GitHub](https://github.com/sanudatta11/statefulmemory): source, issues, and releases
