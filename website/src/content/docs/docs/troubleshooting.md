---
title: Troubleshooting
description: Common memlayer symptoms and fixes.
---

| Symptom | Fix |
|---|---|
| `command not found` | Put `~/.local/bin` on `PATH` |
| Exit 4: daemon down | `memlayer daemon force-start` |
| Exit 5: no project | Run in a git repo, or `export MEMLAYER_PROJECT=…` |
| MCP missing in agent | `memlayer install`, then fully restart the agent |
| Hybrid search empty | `memlayer reindex` |
| Anchored note marked unprovable | Save/verify from a git work tree so digests can be computed |
| Context missing old anchored claims | Expected when verify marked them stale; use `--include-stale` or `verify.serve_stale = true` |
| `decide` / extract / conflict fails | Install an agent CLI on `PATH`, or set `MEMLAYER_LLM_BIN` / `MEMLAYER_LLM_PROVIDER` |
| Git hooks not installed | Run `memlayer install` inside the repo (or omit `--no-git-hooks`) |
| Start over | `memlayer daemon stop && rm -rf ~/.memlayer/` |

```bash
memlayer doctor [--repair]
memlayer logs --lines 50
memlayer daemon status
memlayer verify
```
