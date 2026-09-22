---
title: Decide
description: Ask statefulmemory for a recommendation when project memories conflict; uses your agent CLI for the judge.
---

**Decide** retrieves evidence, surfaces open conflict pairs, and asks a judge to
recommend a resolution. The judge is the local **Laya System-1** sidecar when
`[laya] enabled` (one forward pass, ~100–200 ms on MLX for a batch), falling
back to your coding-agent CLI. Hybrid search stays local; the Laya path needs
no LLM at all.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/decide-mem-v2.mp4" title="Decide and mem archives demo">
  Your browser does not support video.
</video>

## When to use it

- Two decisions disagree (e.g. “SQLite forever” vs “move to Postgres”)
- An agent should pick a side with cited observation ids
- You want an optional recorded follow-up decision note

## Usage

```bash
statefulmemory decide "Should we keep SQLite or move to Postgres?"
statefulmemory decide "Which auth strategy is in force?" --limit 12
```

MCP equivalent: `memory_decide`.

Pin the model if you are using the agent-CLI fallback:

```bash
export STATEFULMEMORY_LLM_PROVIDER=opencode
export STATEFULMEMORY_LLM_MODEL=qwen
# or STATEFULMEMORY_LLM_BIN=/path/to/cursor-agent
```

With Laya enabled, `statefulmemory decide` takes the fast path and no
`STATEFULMEMORY_LLM_*` is needed. Raise `STATEFULMEMORY_LAYA_TIMEOUT_MS=250`
so decide batches (~100–200 ms) fit. See [Laya System-1](/docs/laya/).

## What you get

- A recommendation + short rationale
- Evidence observation ids
- Conflict pair status (`open` / `resolved`) when relations exist

Exact JSON/text fields depend on `--output`; use JSON in pipelines.

## How it fits

Decide reads the same store as [search & context](/docs/search-context/).
Conflict edges come from the supersession / relation graph built when
`conflict.enabled` is on ([Config](/docs/config/)). Record lasting outcomes with
[obs save](/docs/observations/).

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Fails immediately | No Laya and no agent CLI → install OpenCode/Cursor/Claude/Gemini or set `STATEFULMEMORY_LLM_BIN` |
| Thin evidence | Save more notes; raise `--limit`; try a clearer question |
| Want local-only | Enable Laya via `smem install`, or disable `conflict.enabled` / `extract.enabled` and keep hybrid search |
