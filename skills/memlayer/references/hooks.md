# memlayer — lifecycle hooks

`memlayer install` patches `~/.claude/settings.json` with three Claude
Code lifecycle hooks. They run automatically — you do not call the
underlying commands yourself.

## SessionStart — auto-injected briefing

```bash
memlayer obs context --limit 20
```

Runs once at the start of every Claude session. The stdout is piped into
Claude's first turn so prior memory is always visible before the model
reads any code. The output includes:

- `# Last session summary` — most recent rollup from the prior `Stop` hook
- `# Pending review` — decisions whose `review_after` has elapsed
- Recent observations grouped by topic

**Do NOT call `obs context` manually at session start.** It's already
been injected.

## Stop — session rollup

```bash
memlayer session summarize "$CLAUDE_SESSION_ID" --auto
```

Fires when Claude's session ends (including via `/clear` or context
compaction). Groups the session's observations by `topic_key`, picks the
latest per group, and saves an Engram-shaped markdown rollup with
`topic_key="session-summary/<id>"`. The next session's briefing surfaces
it as `# Last session summary`.

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
or daemon failures. **You do not need to call `obs search` separately
for Grep/Read keywords** — the hook does it for you.

## Mid-session focused recall

Re-run `obs context` with a query when you switch tasks:

```bash
memlayer obs context --query "<topic>" --limit 10
```

This is the one case where you call `obs context` manually.
