---
title: Command cheat sheet
description: Quick command index — each line links to the detailed guide.
---

Prefer the feature guides for explanations. This page is a dense index only.

## Install and agents

```bash
make prereqs && make install
memlayer install
memlayer install --agent cursor
memlayer install --no-git-hooks
```

→ [Install](/docs/install/) · [Wire into your agent](/docs/agents/)

## Observations

```bash
memlayer obs save --type decision --title "…" --content "…" --session "$SID"
memlayer obs save --anchor src/auth.rs::login --title "…" --content "…"
memlayer obs recent --limit 10
memlayer obs history <id>
memlayer obs relations <id>
memlayer obs reextract [--since <rfc3339>]
memlayer obs reindex [--force]
```

→ [Observations](/docs/observations/)

## Search and context

```bash
memlayer obs search "auth"
memlayer obs search "auth" --mode bm25
memlayer obs search "auth" --max-tokens 800
memlayer obs context --query "deploy" --limit 20
memlayer obs context --max-tokens 500
memlayer obs context --include-stale
```

→ [Search and context](/docs/search-context/)

## Anchors and verify

```bash
memlayer verify
memlayer verify --quiet
```

→ [Anchors and verify](/docs/anchors-verify/)

## Decide and mem

```bash
memlayer decide "Should we keep SQLite?"
memlayer mem export --out backup.mem
memlayer mem export --out secret.mem --seed-file ./phrase.txt
memlayer mem import backup.mem
```

→ [Decide](/docs/decide/) · [Mem archives](/docs/mem/)

## Ops

```bash
memlayer config show
memlayer daemon status
memlayer doctor [--repair]
memlayer tui
memlayer logs --lines 50
memlayer eval --smoke --save-scorecard card.json
```

→ [Config](/docs/config/) · [LoCoMo eval](/docs/locomo/) · [Troubleshooting](/docs/troubleshooting/)

TTY → text; pipes → JSON. Override with `--output {text,json,yaml}`.
