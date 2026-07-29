# memlayer — failure modes

What to do when something goes wrong with the CLI. **Never** fall back to
the agent's built-in memory.

## Hooks blocked / not running

Symptom: on your first turn there is no memlayer briefing in your context
— no `# Last session summary`, no `# Pending review`, no recent
observations list. Causes:

- **Managed enterprise policy** (most common in Capital One / corporate
  installs): `/Library/Application Support/ClaudeCode/managed-settings.json`
  sets `allowManagedHooksOnly: true` and excludes `SessionStart` / `Stop`.
  User-level and project-level hooks for those events are silently ignored.
  This is not fixable without an IT admin change.
- **Stale session.** Settings were read at session launch; `memlayer
  install` was run later. Restart Claude Code.
- **Project shadowing.** A `<cwd>/.claude/settings.json` defines hooks for
  the same events. `memlayer install` v0.1.0+ merges into it automatically.

**Workaround (always available):** run the underlying CLI commands
yourself.

```bash
# First action of the session — replaces what SessionStart would have done
memlayer obs context --limit 20

# End of session — replaces what Stop would have done
memlayer session summarize "$CLAUDE_SESSION_ID" --auto
```

For PreToolUse Grep/Read nudges, run `obs search "<keyword>"` manually
before introducing new patterns / dependencies.

## CLI not installed (`command not found: memlayer`)

This is the most common state when a user only installed the **skill** files
(e.g. dropped `SKILL.md` into `~/.claude/skills/`) without cloning the repo.
See the "Bootstrap" section of `SKILL.md` — that is the canonical handler.
Quick recap:

1. **Try to install yourself** (works in unsandboxed dev environments):
   ```bash
   git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer \
       && cd ~/Documents/memlayer \
       && cargo build --release -p memlayer-cli \
       && mkdir -p ~/.local/bin \
       && ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer \
       && ~/.local/bin/memlayer install
   ```

2. **If the sandbox blocks `git clone` / `cargo` / network** (managed
   Claude Code installs typically do), do NOT keep retrying. Tell the user
   to run the command themselves — either in their own terminal, or by
   pasting the command into the chat prefixed with `!` so Claude Code
   executes it in this session:
   ```bash
   !git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer && cd ~/Documents/memlayer && cargo build --release -p memlayer-cli && mkdir -p ~/.local/bin && ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer && export PATH="$HOME/.local/bin:$PATH" && ~/.local/bin/memlayer install
   ```

3. **If the repo is already on disk somewhere** (`~/Documents/memlayer`,
   `~/code/memlayer`, `~/src/memlayer`, etc.), skip the clone — just
   build and symlink.

4. **If the user can't or won't install**, warn **once** per session:
   "memlayer unavailable — memory persistence is OFF for this session."
   Then proceed with the task. Do NOT loop / retry. Do NOT use built-in
   or auto-memory as a substitute.

## Daemon unreachable (exit code 4)

The daemon couldn't be reached over the Unix socket.

- Tell the user immediately. Do NOT retry silently.
- Likely a stale socket (the daemon crashed and left `daemon.sock` behind).
  One command clears the stale socket and restarts the daemon:
  ```bash
  memlayer daemon force-start
  ```

## Project name could not be determined (exit code 5)

memlayer auto-detects from `git remote origin` then git-root basename.
If both fail (e.g. not in a git repo), exit 5 fires.

- Tell the user to set `MEMLAYER_PROJECT`:
  ```bash
  export MEMLAYER_PROJECT=my-project
  ```
- Do NOT guess or invent a project name.

## Empty briefing / no observations

Normal for a brand-new project. Proceed without the briefing — there's
nothing to recall yet.

## `obs context` returned the wrong observations

Surface this to the user. Likely a retrieval-quality issue worth filing.
A useful trace can be captured with full audit logging:

```bash
MEMLAYER_AUDIT_FULL=1 memlayer obs context --query "<the query that missed>"
tail -5 ~/.memlayer/queries.log | jq .
```

The log line shows the exact query, the returned observations, and timing.

If BM25 missed a paraphrase (e.g. query "auth" but the obs says "JWT"),
retry with hybrid mode — same query, dense ANN added on top:

```bash
memlayer obs context --query "<the query that missed>" --mode hybrid
```

For stubborn cases, add an LLM rerank pass (5s timeout, falls back to the
hybrid result on error):

```bash
memlayer obs context --query "..." --mode hybrid --rerank haiku
```

## Sandbox blocks the daemon socket

If the agent runs in a restricted sandbox, the Unix socket may be denied
(`Operation not permitted`). The user must allow `~/.memlayer/daemon.sock`
in their sandbox config — `memlayer install` already patches Claude
Code's `settings.json` to allow it.
