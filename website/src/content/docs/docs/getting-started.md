---
title: Getting started
description: Save your first observation and search project memory with memlayer.
---

memlayer captures decisions and fixes in a local SQLite store, then surfaces
them for the next session.

**Prerequisite:** a working CLI — see [Install](/docs/install/) if
`memlayer --version` fails.

## Save an observation

```bash
memlayer obs save \
  --type decision \
  --title "use pgx not GORM" \
  --content "Team prefers raw SQL via pgx" \
  --session "$(uuidgen)"
```

## Anchor to code (optional)

When a note is about a specific symbol, attach one or more anchors. memlayer
stamps the current git commit and a content digest so later `verify` can detect
drift:

```bash
memlayer obs save \
  --type decision \
  --title "login uses JWT" \
  --content "Accept only HS256 in production" \
  --anchor src/auth.rs::login \
  --session "$(uuidgen)"

memlayer verify
```

Context withdraws `stale` / `invalidated` / `unprovable` anchored claims by
default. Pass `--include-stale` or set `verify.serve_stale = true` to keep them.

## Browse and search

```bash
memlayer obs recent --limit 5
memlayer obs search "pgx"
```

## Pull context for a task

```bash
memlayer obs context --query "deploy" --limit 20
memlayer obs context --query "deploy" --max-tokens 500
```

## Decide when notes conflict

```bash
memlayer decide "Should we keep SQLite or move to Postgres?"
```

Needs an agent CLI on `PATH` (or `MEMLAYER_LLM_*`). Hybrid search itself stays
local without an LLM.

## Back up memory

```bash
memlayer mem export --out backup.mem
memlayer mem import backup.mem
```

Data lives under `~/.memlayer/` as plain SQLite you can inspect, back up, or delete.
The daemon auto-starts on first use.

## Next steps

- [Install](/docs/install/) if you have not built the binary yet
- [Wire into your agent](/docs/agents/): MCP / skills so the agent uses memory
- [Everyday commands](/docs/commands/): day-to-day CLI surface
- [LoCoMo eval](/docs/locomo/): smoke, full, and staleness benchmarks
- [GitHub](https://github.com/sanudatta11/memlayer): source and issues
