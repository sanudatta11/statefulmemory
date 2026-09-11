# memlayer — failure modes

What to do when something goes wrong with the CLI. **Never** fall back to
the agent's built-in memory.

## No briefing on the first turn

Symptom: on your first turn there is no memlayer briefing in your context
— no `# Last session summary`, no `# Pending review`, no recent
observations list.

This is expected. This skill does not install or depend on hooks, so
unless the user wired up a `SessionStart` hook themselves, there will be
no auto-injected briefing. It may also be absent because a `/clear`
dropped a prior one, or because managed enterprise policy
(`allowManagedHooksOnly: true`) blocks any user hooks.

**Workaround (always available, and the default for this skill):** run the
CLI commands yourself. This skill never edits `settings.json` or adds
hooks to make this "automatic" — driving it by hand is the intended path.

```bash
# First action of the session — recall prior memory
memlayer obs context --limit 20

# End of session — save a rollup
memlayer session summarize "$CLAUDE_SESSION_ID" --auto
```

Before introducing new patterns / dependencies, run `obs search
"<keyword>"` manually.

## CLI not installed (`command not found: memlayer`)

This is the most common state when a user only installed the **skill** files
(e.g. dropped `SKILL.md` into `~/.claude/skills/`) without cloning the repo.
Building the binary only puts `memlayer` on PATH; it does not touch Claude
configuration. Quick recap:

1. **Try to build yourself** (works in unsandboxed dev environments):
   ```bash
   git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer \
       && cd ~/Documents/memlayer \
       && cargo build --release -p memlayer-cli \
       && mkdir -p ~/.local/bin \
       && ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer
   ```

2. **If the sandbox blocks `git clone` / `cargo` / network** (managed
   Claude Code installs typically do), do NOT keep retrying. Tell the user
   to run the command themselves — either in their own terminal, or by
   pasting the command into the chat prefixed with `!` so Claude Code
   executes it in this session:
   ```bash
   !git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer && cd ~/Documents/memlayer && cargo build --release -p memlayer-cli && mkdir -p ~/.local/bin && ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer && export PATH="$HOME/.local/bin:$PATH"
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
in their own sandbox config. This skill does not modify Claude settings to
grant that — surface the error to the user and let them adjust their
sandbox configuration.
