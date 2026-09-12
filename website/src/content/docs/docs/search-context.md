---
title: Search and context
description: Hybrid BM25 + dense retrieval, token budgets, and packing memory for the next agent turn.
---

Search finds ranked observations. Context packs a working set for a task —
what you hand an agent before it starts coding. Both default to **hybrid**
retrieval: BM25 fused with BGE-small dense vectors via reciprocal rank fusion
(RRF). On the self-host path this does not require a third-party search API
key.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/save-search-context-v2.mp4" title="Save, search, and context demo">
  Your browser does not support video.
</video>

## When to use it

- **Search** — explore what the project already decided (“auth”, “deploy”)
- **Context** — start a task with a bounded memory pack (`--max-tokens`)
- Prefer context in agent hooks / MCP; search for interactive browsing

## Search

```bash
memlayer obs search "auth"                  # hybrid (default)
memlayer obs search "auth" --mode bm25      # lexical only
memlayer obs search "auth" --mode hybrid --rerank
memlayer obs search "auth" --max-tokens 800
memlayer obs search "auth" --all-projects   # include global BM25 mirror
```

| Flag | Purpose |
| --- | --- |
| `--mode` | `hybrid` or `bm25` |
| `--rerank` | Optional LLM rerank (needs agent CLI) |
| `--max-tokens` | Pack results under an estimated token budget |
| `--limit` | Cap hit count before packing |
| `--type` / `--scope` | Filter |

## Context

```bash
memlayer obs context --query "deploy" --limit 20
memlayer obs context --query "deploy" --max-tokens 500
memlayer obs context --query "deploy" --include-stale
```

By default, context **withdraws** observations whose verify state is `stale`,
`invalidated`, or `unprovable`. Unanchored notes are never hidden. Override
with `--include-stale` or `verify.serve_stale = true` (see
[Anchors & verify](/docs/anchors-verify/) and [Config](/docs/config/)).

Config-gated extras (default off): `search.decay_lambda`,
`search.evidence_window`, `search.max_per_type`.

## How it fits

MCP tools `memory_search` and `memory_context` call the same RPCs. Install
wiring: [Wire into your agent](/docs/agents/). Token estimates use a lightweight
heuristic (`tokens_used` in JSON / stderr), not tiktoken.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Hybrid empty, BM25 works | Embeddings missing → `memlayer obs reindex` |
| Context thinner than expected | Anchors went stale → `--include-stale` or re-verify after fixing code |
| Rerank slow / fails | Unset `--rerank` or fix `MEMLAYER_LLM_*` |
