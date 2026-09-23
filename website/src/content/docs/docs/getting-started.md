---
title: Getting started
description: End-to-end tour — install, save, search, anchor, decide, and back up with statefulmemory.
---

This tour walks the main surfaces once. Each step links to a detailed guide.

## 1. Install the CLI

```bash
brew install sanudatta11/tap/statefulmemory   # or build from source:
# git clone https://github.com/sanudatta11/statefulmemory && cd statefulmemory
# make prereqs && make install && export PATH="$HOME/.local/bin:$PATH"
statefulmemory --version
```

`statefulmemory`, `smem`, and `sm` are the same binary — every command in this
tour can be run as `sm <cmd>` for brevity.

Full notes: [Install](/docs/install/). Deployment modes (self-host vs
StatefulMemory Cloud): [Self-hosting and Cloud](/docs/self-hosting/).

## 2. Wire your agent (and optional git hooks)

```bash
statefulmemory install                 # MCP + skills; verify hooks if cwd is a repo
statefulmemory install --no-git-hooks  # skip hooks
```

Details: [Wire into your agent](/docs/agents/).

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/overview-v2.mp4" title="statefulmemory overview">
  Your browser does not support video.
</video>

## 3. Save an observation

```bash
statefulmemory obs save \
  --type decision \
  --title "use pgx not GORM" \
  --content "Team prefers raw SQL via pgx" \
  --session "$(uuidgen)"
```

Guide: [Observations](/docs/observations/).

## 4. Search and pull context

```bash
statefulmemory obs search "pgx"
statefulmemory obs context --query "deploy" --limit 20
statefulmemory obs context --query "deploy" --max-tokens 500
```

Guide: [Search and context](/docs/search-context/).

## 5. Anchor important claims

```bash
statefulmemory obs save \
  --type decision \
  --title "login uses JWT" \
  --content "Accept only HS256 in production" \
  --anchor src/auth.rs::login \
  --session "$(uuidgen)"

statefulmemory verify
```

Guide: [Anchors and verify](/docs/anchors-verify/).

## 6. Decide when notes conflict

```bash
statefulmemory decide "Should we keep SQLite or move to Postgres?"
```

Needs an agent CLI (`STATEFULMEMORY_LLM_*`) or Laya. Guide: [Decide](/docs/decide/).

## 7. Turn on Laya System-1

```bash
smem doctor                    # prints laya enabled / url / health
curl -s http://127.0.0.1:8765/health
# { "ok": true, "backend": "mlx", "loaded": [...], "error": null }
```

`smem install` already wired the sidecar — native **laya-mlx** on Apple Silicon
(~13 ms router P50), PyTorch elsewhere. Decide and conflict take the fast path;
weak calls fall back to your agent CLI. Guide: [Laya System-1](/docs/laya/).

## 8. Back up

```bash
statefulmemory mem export --out backup.mem
```

Guide: [Mem archives](/docs/mem/).

Data lives under `~/.statefulmemory/`. The daemon auto-starts on first use.

## Next

- [Config](/docs/config/) — hybrid, verify, token budget, env overrides
- [Laya System-1](/docs/laya/) — on-device typed decisions, native MLX
- [LoCoMo eval](/docs/locomo/) — smoke, full, staleness
- [Troubleshooting](/docs/troubleshooting/)
- [Command cheat sheet](/docs/commands/)
