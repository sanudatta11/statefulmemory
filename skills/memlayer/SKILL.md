---
name: memlayer
description: "Persistent memory for coding agents — save decisions/patterns/fixes during work, retrieve relevant context at session start. Two CLI calls, per-project scoped."
scope: repo
---

# memlayer — Agent Memory

memlayer stores observations (decisions, patterns, fixes, user feedback) in
a per-project SQLite database and serves them back as markdown for prompt
injection. The agent contract is two CLI calls.

**Use this skill when:**
- Starting work in a project — pull prior context first.
- After making a non-obvious decision (architecture, dependency choice,
  naming convention).
- After a user correction — record what they pushed back on so it sticks.
- After fixing a bug whose root cause might recur.
- Before introducing a new pattern — search for an existing decision first.

**Don't use this skill for:**
- Trivial code edits with no decisions (no memory worth keeping).
- Anything already documented in CLAUDE.md / README / ADRs.
- Per-session ephemeral state (use the conversation, not memory).

## The two-call contract

### 1. Pull context at the start of work

```bash
memlayer obs context --query "<what you're about to work on>" --limit 20
```

Output is markdown — paste it into the system prompt or first user turn.
Add `--output json` if you need structured access. Returns empty if no
observations exist for this project.

If you don't have a specific query yet, omit `--query` to get the most
recent observations across all topics.

### 2. Save observations as you work

```bash
memlayer obs save \
    --type <decision|pattern|fix|feedback|note> \
    --title "<short, searchable>" \
    --content "<why, including any constraints or rejected alternatives>" \
    --session "$SESSION_ID"
```

`$SESSION_ID` should be stable across one agent session. Use whatever the
runtime exposes (e.g., `$CLAUDE_SESSION_ID`) or a `uuidgen` value at start.

### Picking `--type`

| Type | When | Example title |
|---|---|---|
| `decision` | Architectural / tooling choice | "use pgx, not GORM" |
| `pattern` | Convention discovered in the codebase | "errors wrap with `%w` via `fmt.Errorf`" |
| `fix` | Bug fix worth remembering | "nil-deref in handler X caused by uninit map" |
| `feedback` | User correction during a session | "don't add inline comments unless non-obvious" |
| `note` | Anything else worth keeping | "QA env auth uses bearer X" |

### Searching before introducing a new pattern

```bash
memlayer obs search "<keyword>" --limit 10
memlayer obs search "<keyword>" --type fix
```

If you find a relevant prior decision, follow it — don't re-litigate it
unless the user has explicitly asked you to revisit.

## Project scoping

memlayer auto-detects project from `git remote origin` (then git-root
basename). Each project has its own DB; observations don't leak across
repos. Override with `MEMLAYER_PROJECT=<name>` if needed.

## Failure modes

- **CLI not installed**: `memlayer obs context` exits with "command not
  found". Skip silently — don't block work, but warn the user once.
- **No prior observations**: returns empty markdown / empty array. This is
  normal for a new project; just proceed.
- **Daemon failed to spawn** (exit code 4): tell the user, don't retry in
  a loop.

## Behavioral guidance

- **Save sparingly.** Three high-signal observations per session beats
  thirty low-signal ones. If you wouldn't paste the title into a Slack
  message, don't save it.
- **Title is the search key.** Make it specific enough that
  `memlayer obs search "<topic>"` will surface it three months from now.
- **Content is the *why*.** Don't restate the title. Capture the
  constraint or rejected alternative that made the decision non-obvious.
- **Don't save anything you can derive from the codebase.** Architecture
  details belong in the code; conventions belong in linters; only the
  "why we chose X over Y" needs memory.

## See also

- Project README: `README.md` (install + agent integration patterns)
- Full CLI reference: `memlayer --help`
- Roadmap: MCP server (planned) will replace shell-out with native tools.
