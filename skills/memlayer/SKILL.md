---
name: memlayer
description: "Persistent memory for coding agents — save decisions/patterns/fixes during work, retrieve relevant context at session start. CLI-backed, per-project scoped."
scope: repo
---

# memlayer — Agent Memory

**CRITICAL: memlayer CLI is the ONLY memory store. NEVER use the agent's
built-in memory, auto-memory, or any other memory mechanism when this skill
is active.** All memory operations go through `memlayer obs save / search /
context`. If `memlayer` is unavailable, tell the user — do not fall back.

## Bootstrap (run this check FIRST, before any memory operation)

On your very first turn — before saving, searching, or recalling anything —
verify the CLI is installed:

```bash
command -v memlayer >/dev/null && memlayer --version
```

If that succeeds, continue with the rules below. If it fails with `command
not found`, the user has the skill but not the binary. Guide them through a
one-time install — do **not** silently skip memory and do **not** fall back
to built-in memory.

**Preferred path — try the install yourself** (will only work if your
sandbox permits `git clone github.com` and `cargo build`):

```bash
git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer \
    && cd ~/Documents/memlayer \
    && cargo build --release -p memlayer-cli \
    && mkdir -p ~/.local/bin \
    && ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer \
    && ~/.local/bin/memlayer install
```

**Sandboxed path — ask the user to run it.** Most managed/corporate Claude
Code installs deny `cargo` network access or block `git clone` outside an
allowlist. If you hit `Operation not permitted`, `proxy`, or SSL errors,
**stop retrying** and tell the user:

> "memlayer CLI isn't on PATH and my sandbox can't install it. Run the
> command below in your terminal (or paste it into this chat with a leading
> `!` so Claude Code runs it on your behalf), then say 'done':"
>
> ```bash
> git clone https://github.com/sanudatta11/memlayer ~/Documents/memlayer && \
>     cd ~/Documents/memlayer && \
>     cargo build --release -p memlayer-cli && \
>     mkdir -p ~/.local/bin && \
>     ln -sf "$PWD/target/release/memlayer" ~/.local/bin/memlayer && \
>     export PATH="$HOME/.local/bin:$PATH" && \
>     ~/.local/bin/memlayer install
> ```

After the user confirms, re-run `command -v memlayer` once. If it still
fails, warn once: "memlayer unavailable — memory persistence is OFF for
this session." Then proceed without memory. **Never** invent observations
and **never** use built-in / auto-memory as a substitute.

If the repo is already cloned somewhere else (look for `~/Documents/memlayer`,
`~/code/memlayer`, etc.) the build step alone is enough — skip the clone.

## Hard rules (non-negotiable)

- **Check for an auto-injected briefing on your first turn.** Look for a
  "memlayer briefing" / "Last session summary" / `# Pending review` section
  in your initial context. If present, the `SessionStart` hook ran — proceed
  normally and re-run `obs context --query "<topic>"` only when switching
  tasks mid-session.
- **Memory-first on every task — do NOT wait for a hook.** The `SessionStart`
  hook is frequently suppressed (managed enterprise policy blocks user hooks
  via `allowManagedHooksOnly: true`, or a `/clear` drops both this skill and
  the briefing). So before you explore code, search files, or propose a
  fix/implementation for ANY substantive task (fix, implement, debug,
  refactor, "how does X work"), check whether a briefing is already in
  context. If it is NOT, run `memlayer obs context --query "<task>" --limit 20`
  as **step 0** of the task — before any Explore/Grep/Read. The CLI is always
  the source of truth: if a hook can't run, you run the command. Do this once
  per task, not on every message.
- **MUST run `memlayer obs search "<keyword>"` before introducing a new
  pattern, dependency, or convention.** If a prior decision exists, follow
  it unless the user asks to revisit.
- **MUST save an observation after:** non-obvious decisions, user
  corrections / "stop doing X" instructions, bug fixes whose root cause
  might recur, conventions discovered in code that aren't in CLAUDE.md.
  Don't wait for a `Stop` hook to roll up — save observations as they
  happen. The hook (if it fires) only summarizes; it does not replace
  in-flight saves.
- **MUST cite only observations that appeared in `obs context` / `obs
  recent` output.** Never fabricate a memory.
- **MUST NOT save trivial activity** (read file X, ran tests, edited a
  typo). If it wouldn't be Slack-message-worthy, skip it.
- **MUST NOT save anything already in CLAUDE.md / README / commit messages.**
  Memory is for the *why*, not facts that live in code.
- **MUST NOT use built-in agent memory or auto-memory.**

## Save command

```bash
memlayer obs save \
    --type <decision|pattern|fix|feedback|note> \
    --title "<short, searchable>" \
    --content "<why, including constraints / rejected alternatives>" \
    --session "$CLAUDE_SESSION_ID"
```

`$CLAUDE_SESSION_ID` is exposed by Claude Code at runtime. Other agents
should use `uuidgen` once per session.

## See also

- [references/types.md](references/types.md) — picking `--type` (decision /
  pattern / fix / feedback / note)
- [references/slash.md](references/slash.md) — handling `/memlayer <text>`
  and "remember X" requests
- [references/hooks.md](references/hooks.md) — how `SessionStart`, `Stop`,
  and `PreToolUse` hooks auto-wire memlayer to your runtime
- [references/failures.md](references/failures.md) — what to do when CLI
  is missing, daemon is down, or project can't be detected
- [references/examples.md](references/examples.md) — concrete save / search
  / context examples for common scenarios
