---
title: Install
description: Build and install the memlayer CLI on macOS or Linux.
---

**Needs:** Rust stable (≥ 1.75), `protoc`, OpenSSL / `pkg-config`, macOS or Linux (WSL OK).

```bash
git clone https://github.com/sanudatta11/memlayer && cd memlayer
make prereqs && make install    # → ~/.local/bin/memlayer
export PATH="$HOME/.local/bin:$PATH"
memlayer --version
```

The daemon auto-starts on first use. Put `~/.local/bin` on your `PATH` permanently
if `memlayer` is not found after install.

## Wire agents and git hooks

From any project directory (after the binary is on `PATH`):

```bash
memlayer install                 # MCP + skills; git hooks if cwd is a repo
memlayer install --no-git-hooks  # skip post-commit / post-merge / post-checkout
```

When cwd is a git repo, install adds hooks that run `memlayer verify --quiet`
after commit, merge, and checkout. That keeps code-anchored memories marked
when HEAD moves. Uninstall strips the memlayer blocks from those hooks.

`memlayer install` also writes a bootstrap `~/.memlayer/config.toml` (hybrid
search, conflict judge, extract on) without clobbering keys you already set.

## Verify

```bash
memlayer daemon status
memlayer doctor
```

## Next steps

1. [Getting started](/docs/getting-started/): save and search your first notes
2. [Wire into your agent](/docs/agents/): MCP tools and hooks so the agent uses them
3. [GitHub](https://github.com/sanudatta11/memlayer): source, issues, and releases
