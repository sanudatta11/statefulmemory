# memlayer — `/memlayer <text>` slash command

When the user types `/memlayer <anything>`, asks to "save to memlayer", or
says "remember X", treat the text as something to persist via the CLI. Do
NOT interpret it as a query against the agent's internal memory.

## Steps

1. **Classify** the text into `decision | pattern | fix | feedback | note`
   (see [types.md](types.md) for picking).
2. **Derive a short title** (≤ 10 words, searchable).
3. **Run:**
   ```bash
   memlayer obs save \
       --type <type> \
       --title "<derived title>" \
       --content "<full text from user, plus any relevant why/constraints>" \
       --session "$CLAUDE_SESSION_ID"
   ```
4. **Confirm**: tell the user what was saved (type + title).

## Examples

| User says | Type | Title | Content |
|---|---|---|---|
| `/memlayer my project deadline is 20 June` | note | "project deadline: 20 June 2026" | "Project deadline: 20 June 2026 (per user)" |
| `/memlayer prefer pgx over GORM, codegen overhead is a no-go` | decision | "use pgx, not GORM" | "Team prefers raw SQL via pgx; GORM rejected for codegen overhead" |
| `remember don't add inline comments unless non-obvious` | feedback | "no inline comments unless non-obvious" | "User correction during this session: avoid inline comments unless the WHY is non-obvious" |
| `save that as a memlayer observation` (after fixing nil-deref) | fix | "nil-deref in handler X caused by uninit map" | "Bug fix: handler X assumed map was initialized; uninit map caused nil-deref. Fix: initialize map in constructor." |

## Rules

- **No confirmation step.** Save immediately. The user already asked.
- **No fallback to built-in memory.** If `memlayer` CLI is unavailable,
  tell the user — do not silently store anywhere else.
- **Use `$CLAUDE_SESSION_ID`** as the `--session` value. If unset
  (other agents), generate a stable session ID with `uuidgen` once per
  session and reuse it.
