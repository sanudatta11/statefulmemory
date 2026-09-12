---
title: Config
description: memlayer features and configuration overlays.
---

`memlayer install` writes `~/.memlayer/config.toml` with hybrid search, the
conflict judge, and extract enabled. Code defaults also enable
`conflict.enabled` and `search.mode = "hybrid"`. Extract stays opt-in in
code defaults (no LLM on unit tests) but install turns it on.

Hybrid **retrieval** is local (BM25 + BGE-small + RRF). Extract, conflict,
optional `--rerank`, resolve, and Decide call whichever agent CLI is on PATH.
They use **that agent's current/default model** unless you pin one
(`MEMLAYER_LLM_MODEL=qwen`, `--rerank opencode/glm-5.3`, or a concrete Zen id).
Roles in config (`fast` / `capable` / `haiku` / `sonnet`) mean inherit, not a
vendor slug. Host env such as `CURSOR_MODEL` / `OPENCODE_MODEL` is used when set.

Disable the judge if you want heuristic supersession only:

```bash
memlayer config set conflict.enabled false
memlayer config set extract.enabled false    # skip fact triples on save
memlayer config set search.mode bm25         # lexical-only retrieval
memlayer config set embed.quantize true      # int8 vectors (~75% smaller)
memlayer config set verify.serve_stale true  # keep stale claims in context
memlayer config set search.decay_lambda 0.005
memlayer config set search.evidence_window 2
memlayer config set search.max_per_type 2
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
`MEMLAYER_CONFLICT_ENABLED`, `MEMLAYER_CONFLICT_MODEL`, `MEMLAYER_CONFLICT_TIMEOUT_SECS`,
`MEMLAYER_VERIFY_SERVE_STALE`, `MEMLAYER_SEARCH_MODE`,
`MEMLAYER_SEARCH_DECAY_LAMBDA`, `MEMLAYER_SEARCH_EVIDENCE_WINDOW`,
`MEMLAYER_SEARCH_MAX_PER_TYPE`,
`MEMLAYER_LLM_BIN`, `MEMLAYER_LLM_PROVIDER`, `MEMLAYER_LLM_MODEL`
(legacy `MEMLAYER_CLAUDE_MODEL` is the same as `MEMLAYER_LLM_MODEL`).

See the repository [`AGENTS.md`](https://github.com/sanudatta11/memlayer/blob/main/AGENTS.md)
(OpenCode / Codex / local LLMs) and
[`CLAUDE.md`](https://github.com/sanudatta11/memlayer/blob/main/CLAUDE.md)
for the full knob list and architecture notes.
