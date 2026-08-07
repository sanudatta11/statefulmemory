# memlayer — driving memory from the CLI (no hooks, no settings changes)

**This skill does not install lifecycle hooks and does not modify
`~/.claude/settings.json` or any project `settings.json`.** Everything
below is a plain CLI command the agent runs on the hot path. If a briefing
happens to be in your first-turn context, some hook the *user* wired up
separately produced it — this skill neither creates nor requires that.

The rule of thumb: never assume a hook ran. Always drive memory yourself
with the commands here.

## Was a briefing already injected?

On your first turn, look for a memlayer briefing in your context —
`# Last session summary`, `# Pending review`, or a recent-observations
list. Managed enterprise policy commonly blocks user hooks
(`allowManagedHooksOnly: true`), and a `/clear` drops any prior briefing,
so it is often absent.

**If it is absent, run this yourself as your first action** — it is the
CLI equivalent of what a SessionStart hook would have injected:

```bash
memlayer obs context --limit 20
```

**If the briefing is present, do NOT call `obs context` for the first
turn.** Re-run with `--query "<topic>"` later when switching tasks.

## Session start — recall prior memory

```bash
memlayer obs context --limit 20
```

Probes the daemon (respawning it if the socket is stale), then returns
prior memory grouped by topic, plus:

- `# Last session summary` — most recent rollup, if one exists
- `# Pending review` — decisions whose `review_after` has elapsed
- Recent observations grouped by topic

## Session end — save a rollup

Save observations as they happen during the task. At the end, optionally
write a rollup yourself:

```bash
memlayer session summarize "$CLAUDE_SESSION_ID" --auto
```

Groups the session's observations by `topic_key`, picks the latest per
group, and saves an Engram-shaped markdown rollup with
`topic_key="session-summary/<id>"`. The next session's `obs context`
surfaces it as `# Last session summary`.

**Override with agent-written prose** if you have a richer summary:
```bash
echo "## Goal\n...\n## Done\n- ..." \
    | memlayer session summarize "$SESSION_ID" --content -
```
Same `topic_key` → V3 supersession dedups against the auto rollup.

## Before Grep / Read on a new pattern — search first

There is no automatic nudge. Before introducing a new pattern, dependency,
or convention, search prior memory yourself:

```bash
memlayer obs search "<keyword>"
```

## Mid-session focused recall

Re-run `obs context` with a query when you switch tasks:

```bash
memlayer obs context --query "<topic>" --limit 10
```
