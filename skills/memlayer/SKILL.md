---
name: memlayer
description: "Persistent memory for coding agents — save decisions/patterns/fixes during work, retrieve relevant context at session start. Two CLI calls, per-project scoped."
scope: repo
---

# memlayer — Agent Memory

**CRITICAL: memlayer is a CLI-backed memory system. ALL memory operations MUST go through
the `memlayer` CLI. NEVER use the agent's built-in memory, auto-memory, or any other
memory mechanism when this skill is active. The CLI is the only store.**

**When invoked as `/memlayer <text>` or asked to "save to memlayer" or "remember X":**
1. Classify the text as one of: `decision`, `pattern`, `fix`, `feedback`, `note`
2. Run `memlayer obs save` with an appropriate `--title` and `--content`
3. Confirm to the user what was saved

Do NOT call any built-in memory write tool. Do NOT silently store to the agent's
internal memory. If `memlayer` CLI is unavailable, tell the user — do not fall back
to built-in memory.

memlayer stores observations (decisions, patterns, fixes, user feedback) in
a per-project SQLite database and serves them back as markdown for prompt
injection. The agent contract is two CLI calls.

## Hard rules

These are non-negotiable. Treat them as MUST/MUST NOT.

- **At session start, the briefing is auto-injected by the SessionStart hook.**
  You do NOT need to call `obs context` to start a session. Re-run
  `memlayer obs context --query "<topic>"` mid-session when switching tasks.
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
- **MUST NOT use built-in agent memory (auto-memory, internal notes,
  memory tools).** All storage goes through `memlayer obs save`. All
  retrieval goes through `memlayer obs context` / `memlayer obs search`.

## Slash command: /memlayer <text>

When the user types `/memlayer <anything>`, treat the text as something
to persist via the CLI. Do not interpret it as a query to the agent's
internal memory. Follow these steps:

1. **Classify** the text into `decision | pattern | fix | feedback | note`
2. **Derive a short title** (≤ 10 words, searchable)
3. **Run:**
   ```bash
   memlayer obs save \
       --type <type> \
       --title "<derived title>" \
       --content "<full text from user, plus any relevant why/constraints>" \
       --session "$CLAUDE_SESSION_ID"
   ```
4. **Confirm**: tell the user what was saved (type + title)

Example: `/memlayer my project deadline is 20th June`
→ runs `memlayer obs save --type note --title "project deadline: 20 June 2026" --content "..."`

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
  MUST NOT fall back to built-in memory.
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
