---
title: Troubleshooting
description: Common statefulmemory symptoms and fixes.
---

Global matrix. Feature pages also list local tips under **Troubleshooting**.

| Symptom | Fix |
|---|---|
| `command not found` | Put `~/.local/bin` on `PATH` — [Install](/docs/install/) |
| Exit 4: daemon down | `statefulmemory daemon force-start` |
| Exit 5: no project | Run in a git repo, or `export STATEFULMEMORY_PROJECT=…` |
| MCP missing in agent | `statefulmemory install`, then fully restart the agent — [Agents](/docs/agents/) |
| Hybrid search empty | `statefulmemory obs reindex` — [Search and context](/docs/search-context/) |
| Anchored note marked unprovable | Save/verify from a git work tree — [Anchors and verify](/docs/anchors-verify/) |
| Context missing old anchored claims | Expected when verify marked them stale; use `--include-stale` or `verify.serve_stale = true` |
| `decide` / extract / conflict fails | Install an agent CLI on `PATH`, or set `STATEFULMEMORY_LLM_BIN` / `STATEFULMEMORY_LLM_PROVIDER` — [Decide](/docs/decide/) |
| Git hooks not installed | Run `statefulmemory install` inside the repo (or omit `--no-git-hooks`) |
| `.mem` decrypt fails | Wrong seed file / phrase — [Mem archives](/docs/mem/) |
| Start over | `statefulmemory daemon stop && rm -rf ~/.statefulmemory/` |

```bash
statefulmemory doctor [--repair]
statefulmemory logs --lines 50
statefulmemory daemon status
statefulmemory verify
```
