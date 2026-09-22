---
title: Config
description: statefulmemory features and configuration overlays.
---

`smem install` / `statefulmemory install` writes `~/.statefulmemory/config.toml` with
hybrid search, the conflict judge, extract, and **Laya System-1** enabled.
Code defaults also enable `conflict.enabled` and `search.mode = "hybrid"`.
Extract and Laya stay opt-in in code defaults (no LLM / sidecar on unit tests)
but install turns them on. Skip Laya setup with `--no-laya`.

Hybrid **retrieval** is local (BM25 + BGE-small + RRF + optional local CE).
Extract, conflict, optional LLM `--rerank`, resolve, and Decide call whichever
agent CLI is on PATH — or Laya when `[laya] enabled` (silent Claude fallback
on decide/conflict; heuristic-only fallback on the query router).
They use **that agent's current/default model** unless you pin one
(`STATEFULMEMORY_LLM_MODEL=qwen`, `--rerank opencode/glm-5.3`, or a concrete Zen id).
Roles in config (`fast` / `capable` / `haiku` / `sonnet`) mean inherit, not a
vendor slug. Host env such as `CURSOR_MODEL` / `OPENCODE_MODEL` is used when set.

Disable the judge if you want heuristic supersession only:

```bash
statefulmemory config set conflict.enabled false
statefulmemory config set extract.enabled false    # skip fact triples on save
statefulmemory config set search.mode bm25         # lexical-only retrieval
statefulmemory config set embed.quantize true      # int8 vectors (~75% smaller)
statefulmemory config set verify.serve_stale true  # keep stale claims in context
statefulmemory config set search.decay_lambda 0.005
statefulmemory config set search.evidence_window 2
statefulmemory config set search.max_per_type 2
statefulmemory config set laya.enabled false       # disable System-1 sidecar
statefulmemory obs reindex                         # backfill embeddings
```

## Config files

- Global: `~/.statefulmemory/config.toml`
- Per-project overlay: `~/.statefulmemory/projects/<name>.config.toml`

Merge order (highest wins): env vars → project overlay → global → code defaults.

## Common env overrides

`STATEFULMEMORY_EXTRACT_ENABLED`, `STATEFULMEMORY_EXTRACT_MODEL`, `STATEFULMEMORY_EXTRACT_TIMEOUT_SECS`,
`STATEFULMEMORY_EXTRACT_WORKERS`, `STATEFULMEMORY_RERANK_MODEL`, `STATEFULMEMORY_RERANK_TIMEOUT_SECS`,
`STATEFULMEMORY_EMBED_WORKERS`, `STATEFULMEMORY_EMBED_QUANTIZE`,
`STATEFULMEMORY_CONFLICT_ENABLED`, `STATEFULMEMORY_CONFLICT_MODEL`, `STATEFULMEMORY_CONFLICT_TIMEOUT_SECS`,
`STATEFULMEMORY_VERIFY_SERVE_STALE`, `STATEFULMEMORY_SEARCH_MODE`,
`STATEFULMEMORY_SEARCH_DECAY_LAMBDA`, `STATEFULMEMORY_SEARCH_EVIDENCE_WINDOW`,
`STATEFULMEMORY_SEARCH_MAX_PER_TYPE`,
`STATEFULMEMORY_LAYA_ENABLED`, `STATEFULMEMORY_LAYA_URL`, `STATEFULMEMORY_LAYA_TIMEOUT_MS`,
`STATEFULMEMORY_LAYA_ROUTER`, `STATEFULMEMORY_LAYA_DECIDE`, `STATEFULMEMORY_LAYA_CONFLICT`,
`STATEFULMEMORY_LLM_BIN`, `STATEFULMEMORY_LLM_PROVIDER`, `STATEFULMEMORY_LLM_MODEL`
(legacy `STATEFULMEMORY_CLAUDE_MODEL` is the same as `STATEFULMEMORY_LLM_MODEL`).

See the repository [`AGENTS.md`](https://github.com/sanudatta11/statefulmemory/blob/main/AGENTS.md)
(OpenCode / Codex / local LLMs) and
[`CLAUDE.md`](https://github.com/sanudatta11/statefulmemory/blob/main/CLAUDE.md)
for the full knob list and architecture notes.
