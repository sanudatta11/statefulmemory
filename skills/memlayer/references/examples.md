# memlayer — concrete examples

Real-shape examples for the most common flows. Copy / adapt; don't
invent your own command shape.

## Save a decision after a debate

```bash
memlayer obs save \
    --type decision \
    --title "auth: JWT validated in middleware, not per-route" \
    --content "Considered per-route guards but middleware wins because (1) every route needs auth, (2) easier to instrument, (3) avoids drift across handlers. Rejected per-route guards." \
    --session "$CLAUDE_SESSION_ID"
```

## Save a fix where the root cause might recur

```bash
memlayer obs save \
    --type fix \
    --title "nil-deref in handler X caused by uninit map" \
    --content "Handler X assumed config.tags map was initialized; nil map caused panic on first request. Root cause: constructor skipped initialization when no tags configured. Fix: always make(map) in constructor." \
    --session "$CLAUDE_SESSION_ID"
```

## Save user feedback ("stop doing X")

```bash
memlayer obs save \
    --type feedback \
    --title "no inline comments unless non-obvious" \
    --content "User correction: stop adding inline comments that just restate the code. Only comment when the WHY is non-obvious." \
    --session "$CLAUDE_SESSION_ID"
```

## Search before introducing a new pattern

```bash
# Before reaching for a new HTTP client lib:
memlayer obs search "http client" --limit 10

# Before designing a new error type:
memlayer obs search "error" --type pattern --limit 5
```

If a hit comes back, follow it. If not, proceed and save the resulting
decision.

## Mid-session task switch — focused recall

When pivoting from "auth" work to "rate limiting":

```bash
memlayer obs context --query "rate limiting" --limit 10
```

Returns prior observations + briefing scoped to the new topic.

## Pull recent activity for review

```bash
memlayer obs recent --limit 10                    # all types
memlayer obs recent --limit 10 --type decision    # decisions only
memlayer obs get 42                               # full content for one
```

## Inspect via SQL when the CLI is misbehaving

```bash
# Find the project DB:
ls ~/.memlayer/projects/

# Raw query:
sqlite3 ~/.memlayer/projects/<project>.sqlite \
    'SELECT id, type, title, created_at FROM observations
     WHERE deleted_at IS NULL
     ORDER BY id DESC LIMIT 10;'
```

## When the user asks "what did we decide about X"

```bash
memlayer obs search "X" --limit 5
```

Cite only what the search returns. Never paraphrase a memory that didn't
appear in the output.
