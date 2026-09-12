---
title: Why memlayer
description: Engineering comparison — coding-agent memory with self-host and Cloud SaaS vs vector databases alone.
---

Memlayer is **persistent memory infrastructure for coding agents**: CLI + MCP →
gRPC daemon → per-project SQLite (FTS5 + optional hybrid). You can **self-host**
(laptop or team TCP) or use **Memlayer Cloud** (managed SaaS).

This page is the place for an honest engineering comparison. Product UI and
CLI help do not roast peers by name.

## What you get

- Self-host: inspectable per-project SQLite under `~/.memlayer/`
- Cloud SaaS: managed hosting for teams that do not want to run the daemon
- Hybrid BM25 + dense retrieval on the self-host path without a search API key
- Fourteen `memlayer install` targets plus seven MCP tools
- Code anchors + git verify (stale withdrawal from context)
- Optional LLM steps via your agent CLI

## Comparison

| Dimension | Memlayer | Hosted memory APIs | Vector DB alone |
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
settings. Memlayer does **not** claim to beat those numbers until a disclosed
stratified scorecard exists.

How we run and interpret evals: [LoCoMo eval](/docs/locomo/).

## What we do not claim

- No Python / TypeScript Memory SDK today
- No LangGraph / CrewAI / LlamaIndex as shipped integrations
- No Windows support for the CLI v1
- No “beats peer X” until stratified LoCoMo numbers are published

Next: [Architecture](/docs/architecture/) · [Integrations](/docs/integrations/) ·
[Self-hosting and Cloud](/docs/self-hosting/).
