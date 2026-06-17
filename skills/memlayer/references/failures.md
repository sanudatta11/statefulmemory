# memlayer — failure modes

What to do when something goes wrong with the CLI. **Never** fall back to
the agent's built-in memory.

## CLI not installed (`command not found: memlayer`)

- Warn the user **once** per session: "memlayer CLI not on PATH — memory
  persistence disabled for this session."
- Proceed with the task. Do NOT loop / retry.
- Suggest the user run the install:
  ```bash
  cd ~/Documents/memlayer && cargo build --release -p memlayer-cli \
      && ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer
  ```

## Daemon unreachable (exit code 4)

The daemon couldn't be reached over the Unix socket.

- Tell the user immediately. Do NOT retry silently.
- Likely a stale socket. The user should run:
  ```bash
  memlayer daemon status
  rm -f ~/.memlayer/daemon.sock
  memlayer daemon status   # auto-respawns
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

## Sandbox blocks the daemon socket

If the agent runs in a restricted sandbox, the Unix socket may be denied
(`Operation not permitted`). The user must allow `~/.memlayer/daemon.sock`
in their sandbox config — `memlayer install` already patches Claude
Code's `settings.json` to allow it.
