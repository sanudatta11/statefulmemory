---
name: memlayer
description: "Persistent memory for coding agents — save decisions/patterns/fixes during work, retrieve relevant context at session start. Two CLI calls, per-project scoped."
scope: repo
---

# memlayer — Agent Memory

memlayer stores observations (decisions, patterns, fixes, user feedback) in
a per-project SQLite database and serves them back as markdown for prompt
injection. The agent contract is two CLI calls.

## Hard rules

These are non-negotiable. Treat them as MUST/MUST NOT.

- **MUST run `memlayer obs context` at the start of every session** in a
  project, before reading code or making decisions. Inject the result
  into your working context.
- **MUST run `memlayer obs search "<keyword>"` before introducing a new
  pattern, dependency, or convention.** If a prior observation covers
  the same ground, follow it — do not re-litigate unless the user
  explicitly asks you to revisit.
- **MUST save an observation after each of these events:**
    1. A non-obvious decision (architecture, dependency, naming, error
       handling, deployment target, etc.).
    2. A user correction, pushback, or "stop doing X" instruction.
    3. A bug fix whose root cause is non-obvious or might recur.
    4. A discovered convention not already documented in CLAUDE.md or
       similar.
- **MUST cite only observations that actually appeared in `obs context`
  or `obs recent` output.** MUST NOT fabricate or paraphrase a memory
  that isn't in the retrieved set.
- **MUST NOT save trivial activity** (read file X, ran tests, edited a
  typo). If you wouldn't paste the title into a Slack message to a
  teammate, do not save it.
- **MUST NOT save anything already documented in CLAUDE.md, README,
  ADRs, or commit messages.** Memory is for the *why* behind decisions,
  not facts that live in the code.

## The two-call contract

### 1. Pull context at the start of work — REQUIRED

```bash
memlayer obs context --query "<what you're about to work on>" --limit 20
```

Output is markdown. Paste it into your system prompt or first user turn.
Add `--output json` if you need structured access. Empty result is
normal for a new project — proceed without it. Run again with a more
specific `--query` whenever you switch sub-tasks within the session.

If you don't have a specific query yet, omit `--query` to get the most
recent observations across all topics.

### 2. Save observations as you work — REQUIRED for the events listed above

```bash
memlayer obs save \
    --type <decision|pattern|fix|feedback|note> \
    --title "<short, searchable>" \
    --content "<why, including any constraints or rejected alternatives>" \
    --session "$SESSION_ID"
```

`$SESSION_ID` MUST be stable across one agent session. Use whatever the
runtime exposes (e.g., `$CLAUDE_SESSION_ID`) or a `uuidgen` value
captured at session start.

### Picking `--type` — pick exactly one

| Type | When (MUST match this case) | Example title |
|---|---|---|
| `decision` | Architectural / tooling choice with a rejected alternative | "use pgx, not GORM" |
| `pattern` | Convention discovered in the codebase | "errors wrap with `%w` via `fmt.Errorf`" |
| `fix` | Bug fix whose root cause might recur | "nil-deref in handler X caused by uninit map" |
| `feedback` | User correction during a session | "don't add inline comments unless non-obvious" |
| `note` | Anything else worth keeping (use sparingly) | "QA env auth uses bearer X" |

### Searching before introducing a new pattern — REQUIRED

```bash
memlayer obs search "<keyword>" --limit 10
memlayer obs search "<keyword>" --type fix
```

If a relevant prior decision exists, you MUST follow it. The only
exception: the user has explicitly asked you to revisit or override
that decision in the current turn.

## Project scoping

memlayer auto-detects project from `git remote origin` (then git-root
basename). Each project has its own DB; observations do not leak across
repos. If auto-detection fails (exit code 5), set
`MEMLAYER_PROJECT=<name>` in the environment before retrying.

## Failure modes

- **CLI not installed** (`command not found`): warn the user **once**
  per session, then proceed without memlayer. MUST NOT retry in a loop.
- **No prior observations**: empty markdown or empty array. Normal for a
  new project. Proceed.
- **Daemon unreachable** (exit code 4): tell the user immediately, MUST
  NOT retry silently. They likely need to remove a stale socket.
- **Project name could not be determined** (exit code 5): tell the user
  to set `MEMLAYER_PROJECT`. MUST NOT guess a project name.

## Quality bar for what you save

- **Save sparingly.** Three high-signal observations per session beats
  thirty low-signal ones.
- **Title is the search key.** Make it specific enough that running
  `memlayer obs search "<topic>"` three months from now will surface
  it. Bad: "decision about auth". Good: "auth: JWT validation in
  middleware, not per-route".
- **Content is the *why*.** MUST NOT restate the title. Capture the
  constraint, the rejected alternative, or the user feedback that made
  the decision non-obvious.
- **One observation = one decision.** If you find yourself listing
  multiple unrelated points in a single `--content`, split it into
  multiple `obs save` calls.

## See also

- Project README: `README.md` (install + agent integration patterns)
- Full CLI reference: `memlayer --help`
- Roadmap: MCP server (planned) will replace shell-out with native tools.
