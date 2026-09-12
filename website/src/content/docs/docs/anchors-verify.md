---
title: Anchors and verify
description: Tie memories to path::symbol, stamp digests at save, and withdraw stale claims from context.
---

A **code anchor** binds an observation to a symbol in the repo
(`src/auth.rs::login`). On save, memlayer stamps the current git commit and a
content digest. **`memlayer verify`** (and install git hooks) re-check anchors
against HEAD and mark claims `fresh`, `stale`, `invalidated`, or `unprovable`.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/anchors-verify.mp4" title="Anchors and verify demo">
  Your browser does not support video.
</video>

## When to use it

- The note is about a specific function, type, or file region
- You want context to stop serving advice after the code moved
- You work in a git repo (digests need a work tree)

## Save with anchors

```bash
memlayer obs save \
  --type decision \
  --title "login uses JWT" \
  --content "Accept only HS256 in production" \
  --anchor src/auth.rs::login \
  --session "$(uuidgen)"

# Multiple anchors on one observation:
memlayer obs save \
  --title "auth boundary" \
  --content "…" \
  --anchor src/a.rs::foo \
  --anchor src/b.rs::bar \
  --session "$(uuidgen)"
```

## Verify

```bash
memlayer verify
memlayer verify --quiet    # for git hooks / CI
```

| State | Meaning |
| --- | --- |
| fresh | Digest still matches HEAD |
| stale | File/symbol changed since stamp |
| invalidated | Anchor target gone or broken |
| unprovable | Not a git work tree / cannot compute digest |
| unanchored | No anchors (never withdrawn from context) |

## Git hooks

`memlayer install` inside a repo installs `post-commit`, `post-merge`, and
`post-checkout` hooks that run `memlayer verify --quiet`. Skip with
`--no-git-hooks`. Uninstall removes the memlayer blocks.

## Context withdrawal

Default: `verify.serve_stale = false` — context drops stale / invalidated /
unprovable. Search still returns them flagged.

```bash
memlayer obs context --query "auth" --include-stale
memlayer config set verify.serve_stale true
```

## How it fits

Anchors are optional on every [save](/docs/observations/). Agents should prefer
anchored decisions for code-tied claims (see MCP / shell guidance on
[Wire into your agent](/docs/agents/)).

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Always `unprovable` | Run save/verify from a git checkout |
| Hooks missing | `memlayer install` without `--no-git-hooks` |
| Good notes missing from context | Expected after drift; fix code, re-save, or `--include-stale` |
