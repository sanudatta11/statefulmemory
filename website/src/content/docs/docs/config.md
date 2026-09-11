---
title: Config
description: Optional memlayer features and configuration overlays.
---

Optional features are off by default:

```bash
memlayer config set extract.enabled true     # LLM fact triples on save
memlayer config set conflict.enabled true    # LLM supersession judge
memlayer config set embed.quantize true      # int8 vectors (~75% smaller)
memlayer obs reindex                         # backfill embeddings
```

## Config files

- Global: `~/.memlayer/config.toml`
- Per-project overlay: `~/.memlayer/projects/<name>.config.toml`

Merge order (highest wins): env vars → project overlay → global → code defaults.

## Common env overrides

`MEMLAYER_EXTRACT_ENABLED`, `MEMLAYER_EXTRACT_MODEL`, `MEMLAYER_EXTRACT_TIMEOUT_SECS`,
`MEMLAYER_EXTRACT_WORKERS`, `MEMLAYER_RERANK_MODEL`, `MEMLAYER_RERANK_TIMEOUT_SECS`,
`MEMLAYER_EMBED_WORKERS`, `MEMLAYER_EMBED_QUANTIZE`,
`MEMLAYER_CONFLICT_ENABLED`, `MEMLAYER_CONFLICT_MODEL`, `MEMLAYER_CONFLICT_TIMEOUT_SECS`.

See the repository [`CLAUDE.md`](https://github.com/sanudatta11/memlayer/blob/main/CLAUDE.md)
for the full knob list and architecture notes.
