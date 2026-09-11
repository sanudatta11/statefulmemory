# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Primary visitors are developers and operators of AI coding agents (Claude Code, Cursor, Windsurf, and peers) who need durable project memory without sending it to a cloud service. On memlayer.org they both evaluate whether to install and look up install / agent-wiring / CLI answers.

## Product Purpose

memlayer is persistent, local, per-project memory for AI coding agents. It stores decisions, patterns, fixes, and notes in SQLite and surfaces the right ones for the next session via a thin CLI talking to a per-user gRPC daemon (and MCP tools). Success for the public site means a visitor understands the local-first offer in one viewport and can install or wire an agent without leaving the docs.

## Positioning

Local-first agent memory you own on disk under `~/.memlayer/` — not a hosted memory API. The meaningfully different mechanism is a user-local daemon + per-project SQLite (BM25 / optional hybrid) that agents reach through CLI or MCP.

## Operating Context

Used beside coding agents in git repos on macOS/Linux (WSL OK). Rituals: install binary, `memlayer install` for agent MCP/skills, `obs save` / `obs search` / `obs context` during sessions. Docs live at https://memlayer.org; source at github.com/sanudatta11/memlayer.

## Capabilities and Constraints

- Public surface is an Astro Starlight docs site in `website/`, deployed to GitHub Pages with custom domain memlayer.org.
- Product is OSS (MIT OR Apache-2.0); Windows out of scope for the CLI v1.
- Site v1: no analytics; text wordmark (no custom logo mark yet); unversioned docs tracking main.
- Docs content is adapted from the README; internal PRD/roadmap stay out of public nav.
- Open: exact marketing claims beyond documented behavior must not be invented.

## Brand Commitments

- Name: **memlayer** (lowercase wordmark).
- Voice: direct, technical, concrete — CLI/commands over hype.
- Binding references: local / per-project / no cloud; agent integrations listed in README.
- User-approved design defaults: OSS framework-style docs (not heavy SaaS marketing); terminal/local-tool visual direction preferred over generic purple SaaS or cream-editorial defaults.

## Evidence on Hand

- README and `website/src/content/docs/**` — real install and command copy.
- GitHub repo badges and license.
- No customer logos, testimonials, benchmarks, or product screenshots as marketing assets yet — do not fabricate them.

## Product Principles

1. Local ownership is the offer — design and copy must make “on your machine” obvious.
2. Docs serve evaluate *and* lookup — homepage persuades; interior pages prioritize scan and commands.
3. Show the mechanism (CLI/daemon/SQLite), don’t claim nebulous “AI memory.”
4. Stay maintainable inside Starlight — distinctive theme without abandoning docs chrome.
5. Prefer concrete commands and paths over abstract metaphors.

## Accessibility & Inclusion

Follow WCAG 2.2 AA contrast for body text; preserve keyboard navigation and Starlight’s built-in a11y. No product-specific alternate needs recorded yet.
