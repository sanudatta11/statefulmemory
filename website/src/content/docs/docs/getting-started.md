---
title: Getting started
description: End-to-end tour — install, save, search, anchor, decide, and back up with memlayer.
---

This tour walks the main surfaces once. Each step links to a detailed guide.

## 1. Install the CLI

```bash
git clone https://github.com/sanudatta11/memlayer && cd memlayer
make prereqs && make install
export PATH="$HOME/.local/bin:$PATH"
memlayer --version
```

Full notes: [Install](/docs/install/). Deployment modes (self-host vs
Memlayer Cloud): [Self-hosting and Cloud](/docs/self-hosting/).

## 2. Wire your agent (and optional git hooks)

```bash
memlayer install                 # MCP + skills; verify hooks if cwd is a repo
memlayer install --no-git-hooks  # skip hooks
```

Details: [Wire into your agent](/docs/agents/).

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/overview-v2.mp4" title="memlayer overview">
  Your browser does not support video.
</video>

## 3. Save an observation

```bash
memlayer obs save \
  --type decision \
  --title "use pgx not GORM" \
  --content "Team prefers raw SQL via pgx" \
  --session "$(uuidgen)"
```

Guide: [Observations](/docs/observations/).

## 4. Search and pull context

```bash
memlayer obs search "pgx"
memlayer obs context --query "deploy" --limit 20
memlayer obs context --query "deploy" --max-tokens 500
```

Guide: [Search and context](/docs/search-context/).

## 5. Anchor important claims

```bash
memlayer obs save \
  --type decision \
  --title "login uses JWT" \
  --content "Accept only HS256 in production" \
  --anchor src/auth.rs::login \
  --session "$(uuidgen)"

memlayer verify
```

Guide: [Anchors and verify](/docs/anchors-verify/).

## 6. Decide when notes conflict

```bash
memlayer decide "Should we keep SQLite or move to Postgres?"
```

Needs an agent CLI (`MEMLAYER_LLM_*`). Guide: [Decide](/docs/decide/).

## 7. Back up

```bash
memlayer mem export --out backup.mem
```

Guide: [Mem archives](/docs/mem/).

Data lives under `~/.memlayer/`. The daemon auto-starts on first use.

## Next

- [Config](/docs/config/) — hybrid, verify, token budget, env overrides
- [LoCoMo eval](/docs/locomo/) — smoke, full, staleness
- [Troubleshooting](/docs/troubleshooting/)
- [Command cheat sheet](/docs/commands/)
