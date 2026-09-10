## memlayer memory protocol

IMPORTANT: memlayer is the ONLY memory store for this agent. You MUST NOT use
built-in memory, auto-memory, internal notes, or any other memory mechanism.
All memory reads and writes go through the `memlayer` CLI exclusively.

The binary is at `~/.local/bin/memlayer` (also `/usr/local/bin/memlayer`).
If `memlayer` is not on PATH, use the full path: `~/.local/bin/memlayer`.

At the start of every session memlayer's SessionStart hook automatically
injects the briefing (last session summary + decisions due for review +
recent observations). You do NOT need to run `obs context` first. Re-run
`memlayer obs context --query "<topic>"` mid-session when switching tasks.

Before any Grep or Read tool call, memlayer's PreToolUse hooks run
`memlayer hook pre-tool` automatically. If matching prior observations
exist, Grep mode injects them as additional context (markdown to stdout)
and Read mode prints a stderr hint. You do NOT need to manually run
`obs search` before grepping — the hook does it for you. The hook is
silent on misses, capped at 500 ms, and never blocks the tool call.

Before introducing any new pattern, dependency, library, or naming convention, you MUST run:
  memlayer obs search "<keyword>"

After each of the following events, you MUST save an observation — no exceptions:
- A non-obvious architectural or tooling decision (include rejected alternatives)
- A user correction, pushback, or instruction to stop doing something
- A bug fix whose root cause is non-obvious or might recur
- A discovered convention not already in project documentation

  memlayer obs save \
      --type <decision|pattern|fix|feedback|note> \
      --title "<short, searchable title>" \
      --content "<why — include rejected alternatives and constraints>" \
      --session "$SESSION_ID"

When the user types `/memlayer <text>` or says "remember X" or "save to memlayer":
  Run `obs save` immediately. Do NOT use built-in memory. Do NOT ask for confirmation.

You MUST NOT cite memories that were not returned by `obs context` or `obs recent`.
You MUST NOT save trivial activity such as reading files, running tests, or editing typos.
If the memlayer CLI is unavailable: warn the user once and proceed WITHOUT any memory storage.
Do NOT fall back to built-in or auto-memory under any circumstances.
