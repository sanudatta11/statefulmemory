---
title: Why statefulmemory
description: Engineering comparison — coding-agent memory with self-host and Cloud SaaS vs vector databases alone.
---

StatefulMemory is **persistent memory infrastructure for coding agents**: CLI + MCP →
gRPC daemon → per-project SQLite (FTS5 + optional hybrid). You can **self-host**
(laptop or team TCP) or use **StatefulMemory Cloud** (managed SaaS).

This page is the place for an honest engineering comparison. Product UI and
CLI help do not roast peers by name.

## What you get

- Self-host: inspectable per-project SQLite under `~/.statefulmemory/`
- Cloud SaaS: managed hosting for teams that do not want to run the daemon
- Hybrid BM25 + dense retrieval on the self-host path without a search API key
- Fourteen `statefulmemory install` targets plus seven MCP tools
  (`statefulmemory` / `smem` / `sm` are the same binary)
- Code anchors + git verify (stale withdrawal from context)
- Optional LLM steps via your agent CLI
- **Effective context** (`smem context compile`): pack decisions / anchors / facts
  into a fixed host window (`8k`…`1m`) so a 200k or 1M agent acts like a much
  larger working set — durable store stays outside the KV cache
- Repo ingest (`smem ingest repo`): path-anchored observations from git-tracked files
- Opt-in entity graph with BFS or HippoRAG-style PPR ranking (`graph.ranker = ppr`)
- **Wave 3 retrieve:** adaptive Easy/Normal/Hard router, local MiniLM-style CE
  rerank (default, LLM-free), result LRU cache, index-time fact key expand
- **Laya System-1 (opt-in):** on-device typed decisions for router / decide /
  conflict — native MLX on Apple Silicon, PyTorch elsewhere. Not a reranker;
  soft-fails to heuristic (router) or agent CLI (decide/conflict).
  See `tools/laya-sidecar/` and the [Laya guide](/docs/laya/).

## Effective context (200k / 1M ≈ 100M with memory)

Long-context models still pay attention cost inside the window. StatefulMemory
keeps the corpus in SQLite and **compiles** a slot-budgeted brief (≈35%
decisions, ≈35% anchored code, ≈30% related) for whatever window the host
exposes. Retrieval borrows LongMemEval indexing tips (fact-augmented query
merge, heuristic time pruning) and optional Personalized PageRank over the
entity graph for single-step multi-hop — without claiming Magic-style 100M
trained LTM weights.

Unmatched-harness caveat: this is a systems claim (more useful tokens per
window), not a published LoCoMo/LongMemEval scorecard win.

## Laya System-1: decisions, not text

The optional Laya sidecar answers the discrete questions the memory layer asks —
which retrieval tier a query needs, what Decide should recommend, whether two
memories conflict — in one non-autoregressive forward pass. On Apple Silicon it
runs native **laya-mlx** (published M3 Max P50 **13.4 ms** for the 421M
checkpoint, no PyTorch); elsewhere it uses the PyTorch `laya` package. Same
checkpoints, same typed schema. Weak calls soft-fail to heuristics or your
agent CLI, so search and context keep working with Laya off.

Numbers are from the `laya-mlx` project's published benchmarks and are not a
statefulmemory-harness score. See [Laya System-1](/docs/laya/).

## Comparison

| Dimension | StatefulMemory | Hosted memory APIs | Vector DB alone |
|---|---|---|---|
| Fully local / self-host save / search / context | ✓ | Often no | If you self-host |
| Managed Cloud SaaS | ✓ (offering) | Yes | N/A |
| Coding-agent install (MCP / skills) | ✓ (14 targets) | Rare | No |
| Inspectable per-project SQLite (self-host) | ✓ | No | N/A |
| Hybrid BM25 + dense on self-host | ✓ | Varies | Vectors only |
| Code anchors + git verify | ✓ | No | No |
| Python Memory SDK | Not yet | Often yes | Client libs |

## Peer benchmarks (caveats)

Published LoCoMo-style LLM-judge numbers for other systems (for example
**Mem0 ~66.9%**, Engram-family bands roughly **68–80%**) come from
**unmatched harnesses** — different judges, category filters, and retrieval
settings. StatefulMemory does **not** claim to beat those numbers until a disclosed
stratified scorecard exists.

How we run and interpret evals: [LoCoMo eval](/docs/locomo/).

## What we do not claim

- No Python / TypeScript Memory SDK today
- No LangGraph / CrewAI / LlamaIndex as shipped integrations
- No Windows support for the CLI v1
- No “beats peer X” until stratified LoCoMo numbers are published

Next: [Architecture](/docs/architecture/) · [Integrations](/docs/integrations/) ·
[Self-hosting and Cloud](/docs/self-hosting/).
