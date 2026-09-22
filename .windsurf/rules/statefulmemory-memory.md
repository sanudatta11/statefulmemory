## statefulmemory memory protocol

IMPORTANT: statefulmemory is the ONLY memory store for this agent. You MUST NOT use
built-in memory, auto-memory, internal notes, or any other memory mechanism.
All memory reads and writes go through the `statefulmemory` CLI exclusively.

The binary is at `~/.local/bin/statefulmemory` (also `/usr/local/bin/statefulmemory`).
If `statefulmemory` is not on PATH, use the full path: `~/.local/bin/statefulmemory`.

At the start of every session statefulmemory's SessionStart hook automatically
injects the briefing (last session summary + decisions due for review +
recent observations) — **if the hook is allowed to run**. Look for a
"statefulmemory briefing" / "Last session summary" / "Pending review" section
in your initial context.

If you see the briefing: do NOT run `obs context` first. Re-run
`statefulmemory obs context --query "<topic>"` mid-session when switching tasks.

Do NOT rely on the briefing appearing — the SessionStart hook is often
suppressed (managed `allowManagedHooksOnly: true`, or after a `/clear`).
Treat the memory check as keyed off the TASK, not the session: before you
explore code, search files, or propose a fix/implementation for any
substantive task (fix, implement, debug, refactor, "how does X work"), if
no briefing is already in context, run
`statefulmemory obs context --query "<task>" --limit 20` as step 0 — before any
Grep/Read/exploration. Do this once per task, not on every message.

Before any Grep or Read tool call, statefulmemory's PreToolUse hooks run
`statefulmemory hook pre-tool` automatically. If matching prior observations
exist, Grep mode injects them as additional context (markdown to stdout)
and Read mode prints a stderr hint. You do NOT need to manually run
`obs search` before grepping — the hook does it for you. The hook is
silent on misses, capped at 500 ms, and never blocks the tool call.

Before introducing any new pattern, dependency, library, or naming convention, you MUST run:
  statefulmemory obs search "<keyword>"

After each of the following events, you MUST save an observation — no exceptions:
- A non-obvious architectural or tooling decision (include rejected alternatives)
- A user correction, pushback, or instruction to stop doing something
- A bug fix whose root cause is non-obvious or might recur
- A discovered convention not already in project documentation

  statefulmemory obs save \
      --type <decision|pattern|fix|feedback|note> \
      --title "<short, searchable title>" \
      --content "<why — include rejected alternatives and constraints>" \
      --session "$SESSION_ID"

When the user types `/statefulmemory <text>` or says "remember X" or "save to statefulmemory":
  Run `obs save` immediately. Do NOT use built-in memory. Do NOT ask for confirmation.

You MUST NOT cite memories that were not returned by `obs context` or `obs recent`.
You MUST NOT save trivial activity such as reading files, running tests, or editing typos.
If the statefulmemory CLI is unavailable (`command not found: statefulmemory`): the user
likely installed the skill/rule files without the binary. Walk them through
the one-time install — clone, build, symlink, then run `statefulmemory install`:

  git clone https://github.com/sanudatta11/statefulmemory ~/Documents/statefulmemory && \
      cd ~/Documents/statefulmemory && cargo build --release -p statefulmemory-cli && \
      mkdir -p ~/.local/bin && \
      ln -sf "$PWD/target/release/statefulmemory" ~/.local/bin/statefulmemory && \
      ~/.local/bin/statefulmemory install

Try it yourself first. If your sandbox denies `git clone github.com` or
`cargo` network access (common in managed Claude Code installs), do NOT
keep retrying — ask the user to paste the command into chat with a leading
`!` so Claude Code runs it on their behalf, or to run it in their own
terminal. After they confirm, re-check `command -v statefulmemory`. If it still
fails, warn the user once and proceed WITHOUT any memory storage.
Do NOT fall back to built-in or auto-memory under any circumstances.
