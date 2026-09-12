---
title: Observations
description: Save, list, and manage persistent notes — decisions, facts, patterns, and fixes in local SQLite.
---

Observations are the unit of memory in memlayer: a typed note with a title,
content, optional session, and optional code anchors. They live in per-project
SQLite under `~/.memlayer/` and are searchable with BM25 and dense vectors.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/save-search-context.mp4" title="Save, search, and context demo">
  Your browser does not support video.
</video>

## When to use it

- After a decision, bug fix, or correction you want the next session to see
- When an agent should stop rediscovering the same preference
- Before ending a coding session (batch a few high-value notes)

## Save

```bash
memlayer obs save \
  --type decision \
  --title "use pgx not GORM" \
  --content "Team prefers raw SQL via pgx for this service" \
  --session "$(uuidgen)"
```

| Flag | Purpose |
| --- | --- |
| `--type` | `decision`, `fact`, `pattern`, `fix`, `note`, … |
| `--title` | Short label (shown in lists and search) |
| `--content` | Body; pass `-` to read from stdin |
| `--session` | Session id (create one with `memlayer session start` or `uuidgen`) |
| `--topic` | Topic key for supersession / grouping |
| `--anchor` | Repeatable `path::symbol` (see [Anchors & verify](/docs/anchors-verify/)) |
| `--scope` | Usually `project` |

## Browse

```bash
memlayer obs recent --limit 10
memlayer obs get <id-or-sync-id>
memlayer obs history <id>       # supersession chain
memlayer obs relations <id>     # graph edges (conflicts, supersedes, …)
```

## Background maintenance

```bash
memlayer obs reextract [--since <rfc3339>]   # re-queue fact extraction (needs LLM CLI)
memlayer obs reindex [--force]               # re-embed / quantize vectors
```

## How it fits

Saved observations feed [search & context](/docs/search-context/),
[Decide](/docs/decide/), and MCP `memory_add` / `memory_recent`. Anchored ones
are checked by [verify](/docs/anchors-verify/). Bulk backup uses
[`.mem` archives](/docs/mem/).

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Exit 5: no project | Run inside a git repo or set `MEMLAYER_PROJECT` |
| Save succeeds but search empty | Wait for embed worker, or `memlayer obs reindex` |
| Duplicate titles keep stacking | Enable `conflict.enabled` or use a shared `--topic` |
