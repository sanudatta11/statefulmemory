# memlayer — lifecycle hooks

`memlayer install` patches `~/.claude/settings.json` with three Claude
Code lifecycle hooks. They run automatically when the runtime allows it.

## When hooks DON'T run

Hooks can be silently disabled, most commonly by:

- **Enterprise policy.** `/Library/Application Support/ClaudeCode/managed-settings.json`
  (Capital One, other managed installs) often sets `allowManagedHooksOnly: true`
  and only allows `PreToolUse` / `PostToolUse` / `UserPromptSubmit`. In that
  configuration, `SessionStart` and `Stop` are blocked entirely — your hooks
  never fire, and there's no error message.
- **Stale session.** Settings are read once at session launch. If
  `memlayer install` ran after the session started, hooks won't activate
  until the next restart.
- **Project-level shadowing.** `<cwd>/.claude/settings.json` replaces
  global hook entries for the same event. `memlayer install` >= v0.1.0
  merges into the project file when it exists; older versions don't.

**The signal that hooks didn't run:** on your first turn you don't see a
memlayer briefing — no `# Last session summary`, no `# Pending review`, no
recent observations list. If you don't see those, run `obs context`
yourself as your first action:

```bash
memlayer obs context --limit 20
```

The same logic applies to the `Stop` hook: if it can't run, you save
observations as they happen instead of relying on an end-of-session rollup.

## SessionStart — auto-injected briefing (when allowed)

```bash
memlayer obs context --limit 20
```

Runs once at the start of every Claude session. The stdout is piped into
Claude's first turn so prior memory is always visible before the model
reads any code. The output includes:

- `# Last session summary` — most recent rollup from the prior `Stop` hook
- `# Pending review` — decisions whose `review_after` has elapsed
- Recent observations grouped by topic

**If the briefing is present, do NOT call `obs context` manually for the
first turn.** Re-run with `--query "<topic>"` later when switching tasks.

## Stop — session rollup (when allowed)

```bash
memlayer session summarize "$CLAUDE_SESSION_ID" --auto
```

Fires when Claude's session ends (including via `/clear` or context
compaction). Groups the session's observations by `topic_key`, picks the
latest per group, and saves an Engram-shaped markdown rollup with
`topic_key="session-summary/<id>"`. The next session's briefing surfaces
it as `# Last session summary`.

**If the Stop hook is blocked** (enterprise policy, etc.), save each
significant decision/fix/pattern with `obs save` as it happens. Without
the hook there is no rollup — but individual observations still show up
in `obs recent` and `obs search` on the next session.

**Override with agent-written prose** if you have a richer summary:
```bash
echo "## Goal\n...\n## Done\n- ..." \
    | memlayer session summarize "$SESSION_ID" --content -
```
Same `topic_key` → V3 supersession dedups against the auto rollup.

## PreToolUse — Grep / Read nudges

Before the agent calls `Grep` or `Read`, memlayer runs:

```bash
memlayer hook pre-tool --tool Grep --pattern "<text>"
memlayer hook pre-tool --tool Read --path "<path>"
```

- **Grep mode**: if memlayer has matching prior observations, the hook
  prints a markdown block to stdout that Claude sees as additional
  context. The Grep then runs normally.
- **Read mode**: if matches exist, the hook prints a stderr hint
  (e.g. `memlayer hint: 3 prior observations match write — run \`memlayer
  obs search "write"\``). Bodies are NOT injected.

The hook always exits 0, has a 500 ms hard cap, and is silent on misses
or daemon failures. **When the hook runs**, you do not need to call
`obs search` separately for Grep/Read keywords. **When it doesn't run**
(uncommon — PreToolUse is allowed by most managed policies), run
`obs search "<keyword>"` yourself before introducing a new pattern.

## Mid-session focused recall

Re-run `obs context` with a query when you switch tasks, regardless of
whether SessionStart hook ran:

```bash
memlayer obs context --query "<topic>" --limit 10
```
