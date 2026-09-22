---
title: Command cheat sheet
description: Quick command index — each line links to the detailed guide.
---

Prefer the feature guides for explanations. This page is a dense index only.

Every command below works identically as `statefulmemory <cmd>`, `smem <cmd>`,
or `sm <cmd>` — three names, one binary.

## Install and agents

```bash
make prereqs && make install
statefulmemory install
statefulmemory install --agent cursor
statefulmemory install --no-git-hooks
```

→ [Install](/docs/install/) · [Wire into your agent](/docs/agents/)

## Observations

```bash
statefulmemory obs save --type decision --title "…" --content "…" --session "$SID"
statefulmemory obs save --anchor src/auth.rs::login --title "…" --content "…"
statefulmemory obs recent --limit 10
statefulmemory obs history <id>
statefulmemory obs relations <id>
statefulmemory obs reextract [--since <rfc3339>]
statefulmemory obs reindex [--force]
```

→ [Observations](/docs/observations/)

## Search and context

```bash
statefulmemory obs search "auth"
statefulmemory obs search "auth" --mode bm25
statefulmemory obs search "auth" --max-tokens 800
statefulmemory obs context --query "deploy" --limit 20
statefulmemory obs context --max-tokens 500
statefulmemory obs context --include-stale
```

→ [Search and context](/docs/search-context/)

## Anchors and verify

```bash
statefulmemory verify
statefulmemory verify --quiet
```

→ [Anchors and verify](/docs/anchors-verify/)

## Decide and mem

```bash
statefulmemory decide "Should we keep SQLite?"
statefulmemory mem export --out backup.mem
statefulmemory mem export --out secret.mem --seed-file ./phrase.txt
statefulmemory mem import backup.mem
```

→ [Decide](/docs/decide/) · [Mem archives](/docs/mem/)

## Ops

```bash
statefulmemory config show
statefulmemory daemon status
statefulmemory doctor [--repair]
statefulmemory tui
statefulmemory logs --lines 50
statefulmemory eval --smoke --save-scorecard card.json
```

→ [Config](/docs/config/) · [LoCoMo eval](/docs/locomo/) · [Troubleshooting](/docs/troubleshooting/)

TTY → text; pipes → JSON. Override with `--output {text,json,yaml}`.
