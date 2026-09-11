---
title: Troubleshooting
description: Common memlayer symptoms and fixes.
---

| Symptom | Fix |
|---|---|
| `command not found` | Put `~/.local/bin` on `PATH` |
| Exit 4 — daemon down | `memlayer daemon force-start` |
| Exit 5 — no project | Run in a git repo, or `export MEMLAYER_PROJECT=…` |
| MCP missing in agent | `memlayer install`, then fully restart the agent |
| Hybrid search empty | `memlayer reindex` |
| Start over | `memlayer daemon stop && rm -rf ~/.memlayer/` |

```bash
memlayer doctor [--repair]
memlayer logs --lines 50
memlayer daemon status
```
