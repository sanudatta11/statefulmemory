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

## Browse and search

```bash
memlayer obs recent --limit 5
memlayer obs search "pgx"
```

## Pull context for a task

```bash
memlayer obs context --query "deploy" --limit 20
```

Data lives under `~/.memlayer/` as plain SQLite you can inspect, back up, or delete.
The daemon auto-starts on first use.

## Next steps

- [Wire into your agent](/docs/agents/) — MCP / skills so the agent uses memory
- [Everyday commands](/docs/commands/) — day-to-day CLI surface
- [GitHub](https://github.com/sanudatta11/memlayer) — source and issues
