---
name: memlayer
description: "Persistent memory for coding agents — save decisions/patterns/fixes during work, retrieve relevant context at session start. CLI-backed, per-project scoped."
scope: repo
---

# memlayer — Agent Memory

**CRITICAL: memlayer is the ONLY memory store.** Prefer MCP tools
(`memory_search`, `memory_add`, `memory_context`, `memory_recent`,
`memory_facts`, `memory_health`) when connected; else use the `memlayer`
CLI. Never fall back to built-in / auto-memory — see
[references/failures.md](references/failures.md) if the CLI is missing.

## Bootstrap (first turn, before any memory op)

```bash
command -v memlayer >/dev/null && memlayer --version
```

If missing, follow [references/failures.md](references/failures.md)
("CLI not installed"). This skill never edits Claude settings or installs
hooks — see [references/hooks.md](references/hooks.md).

## Hard rules

- **Memory-first:** if no briefing is already in context, run
  `memlayer obs context --query "<task>" --limit 20` before Explore/Grep/Read.
- **MUST** `memlayer obs search "<keyword>"` before new patterns/deps/conventions.
- **MUST save** after non-obvious decisions, user corrections, recurring fixes,
  and code conventions not in CLAUDE.md — use `obs save` as they happen.
- **MUST cite only** hits from `obs context` / `obs recent`. Never fabricate.
- **MUST NOT** save trivial activity or facts already in CLAUDE.md / README /
  commits. **MUST NOT** use built-in agent memory.

## Save command

```bash
memlayer obs save \
    --type <decision|pattern|fix|feedback|note> \
    --title "<short, searchable>" \
    --content "<why, including constraints / rejected alternatives>" \
    --session "$CLAUDE_SESSION_ID"
```

`$CLAUDE_SESSION_ID` is Claude Code's runtime id; other agents: `uuidgen`
once per session. Types: [references/types.md](references/types.md).
"Remember X" / slash: [references/slash.md](references/slash.md).
Examples: [references/examples.md](references/examples.md).
