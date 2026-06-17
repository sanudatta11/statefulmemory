---
name: memlayer
description: "Persistent memory for coding agents — save decisions/patterns/fixes during work, retrieve relevant context at session start. CLI-backed, per-project scoped."
scope: repo
---

# memlayer — Agent Memory

**CRITICAL: memlayer CLI is the ONLY memory store. NEVER use the agent's
built-in memory, auto-memory, or any other memory mechanism when this skill
is active.** All memory operations go through `memlayer obs save / search /
context`. If `memlayer` is unavailable, tell the user — do not fall back.

## Hard rules (non-negotiable)

- **Briefing auto-injects at session start via `SessionStart` hook.** Do NOT
  call `obs context` first turn. Re-run `obs context --query "<topic>"`
  mid-session when switching tasks.
- **MUST run `memlayer obs search "<keyword>"` before introducing a new
  pattern, dependency, or convention.** If a prior decision exists, follow
  it unless the user asks to revisit.
- **MUST save an observation after:** non-obvious decisions, user
  corrections / "stop doing X" instructions, bug fixes whose root cause
  might recur, conventions discovered in code that aren't in CLAUDE.md.
- **MUST cite only observations that appeared in `obs context` / `obs
  recent` output.** Never fabricate a memory.
- **MUST NOT save trivial activity** (read file X, ran tests, edited a
  typo). If it wouldn't be Slack-message-worthy, skip it.
- **MUST NOT save anything already in CLAUDE.md / README / commit messages.**
  Memory is for the *why*, not facts that live in code.
- **MUST NOT use built-in agent memory or auto-memory.**

## Save command

```bash
memlayer obs save \
    --type <decision|pattern|fix|feedback|note> \
    --title "<short, searchable>" \
    --content "<why, including constraints / rejected alternatives>" \
    --session "$CLAUDE_SESSION_ID"
```

`$CLAUDE_SESSION_ID` is exposed by Claude Code at runtime. Other agents
should use `uuidgen` once per session.

## See also

- [references/types.md](references/types.md) — picking `--type` (decision /
  pattern / fix / feedback / note)
- [references/slash.md](references/slash.md) — handling `/memlayer <text>`
  and "remember X" requests
- [references/hooks.md](references/hooks.md) — how `SessionStart`, `Stop`,
  and `PreToolUse` hooks auto-wire memlayer to your runtime
- [references/failures.md](references/failures.md) — what to do when CLI
  is missing, daemon is down, or project can't be detected
- [references/examples.md](references/examples.md) — concrete save / search
  / context examples for common scenarios
