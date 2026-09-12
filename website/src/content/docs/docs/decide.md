---
title: Decide
description: Ask memlayer for a recommendation when project memories conflict; uses your agent CLI for the judge.
---

**Decide** retrieves evidence, surfaces open conflict pairs, and asks your
coding-agent CLI to recommend a resolution. Hybrid search stays local; the
synthesis step needs an LLM backend on `PATH` (or `MEMLAYER_LLM_*`).

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/decide-mem-v2.mp4" title="Decide and mem archives demo">
  Your browser does not support video.
</video>

## When to use it

- Two decisions disagree (e.g. “SQLite forever” vs “move to Postgres”)
- An agent should pick a side with cited observation ids
- You want an optional recorded follow-up decision note

## Usage

```bash
memlayer decide "Should we keep SQLite or move to Postgres?"
memlayer decide "Which auth strategy is in force?" --limit 12
```

MCP equivalent: `memory_decide`.

Pin the model if needed:

```bash
export MEMLAYER_LLM_PROVIDER=opencode
export MEMLAYER_LLM_MODEL=qwen
# or MEMLAYER_LLM_BIN=/path/to/cursor-agent
```

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
| Fails immediately | No agent CLI → install OpenCode/Cursor/Claude/Gemini or set `MEMLAYER_LLM_BIN` |
| Thin evidence | Save more notes; raise `--limit`; try a clearer question |
| Want local-only | Do not use Decide; disable `conflict.enabled` / `extract.enabled` and keep hybrid search |
