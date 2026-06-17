# memlayer — picking `--type`

Pick exactly one type per observation. The type affects ranking,
filtering, and section grouping in the auto session-summary rollup.

| Type | When (MUST match this case) | Example title |
|---|---|---|
| `decision` | Architectural / tooling choice with a rejected alternative | "use pgx, not GORM" |
| `pattern` | Convention discovered in the codebase | "errors wrap with `%w` via `fmt.Errorf`" |
| `fix` | Bug fix whose root cause might recur | "nil-deref in handler X caused by uninit map" |
| `feedback` | User correction during a session | "don't add inline comments unless non-obvious" |
| `note` | Anything else worth keeping (use sparingly) | "QA env auth uses bearer X" |

## Quality bar

- **Save sparingly.** Three high-signal observations per session beats
  thirty low-signal ones.
- **Title is the search key.** Make it specific enough that running
  `memlayer obs search "<topic>"` three months from now will surface it.
  Bad: `"decision about auth"`. Good: `"auth: JWT validation in
  middleware, not per-route"`.
- **Content is the *why*.** MUST NOT restate the title. Capture the
  constraint, the rejected alternative, or the user feedback that made
  the decision non-obvious.
- **One observation = one decision.** If you find yourself listing
  multiple unrelated points in a single `--content`, split it into
  multiple `obs save` calls.

## Searching before introducing a new pattern — REQUIRED

```bash
memlayer obs search "<keyword>" --limit 10
memlayer obs search "<keyword>" --type fix
```

If a relevant prior decision exists, you MUST follow it. The only
exception: the user has explicitly asked you to revisit or override it
in the current turn.
