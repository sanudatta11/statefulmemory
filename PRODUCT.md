# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Primary visitors are developers and operators of AI coding agents (Claude Code, Cursor, Windsurf, and peers) who need durable project memory. They may prefer self-host (data on their machines) or Memlayer Cloud (managed SaaS). On memlayer.org they evaluate the offer and look up install / agent-wiring / CLI answers.

## Product Purpose

memlayer is persistent memory infrastructure for coding agents: thin CLI + MCP → gRPC daemon → per-project SQLite (FTS5 + optional hybrid). It stores decisions, patterns, fixes, and notes and surfaces the right ones for the next session. **Self-host** (laptop UDS or team TCP+TLS) and **Memlayer Cloud** (managed SaaS) are both product offerings. Success for the public site means a visitor understands both paths in one viewport and can install or wire an agent without leaving the docs.

## Positioning

Coding-agent memory with a real open-source self-host path **and** a Cloud SaaS path — not a bare vector DB, and not a Python Memory SDK yet (agents use MCP or shell-out). Mechanism on self-host: daemon + per-project SQLite (BM25 / optional hybrid). Cloud: same client surfaces, managed hosting. Engineering comparison belongs on [Why memlayer](https://memlayer.org/docs/why-memlayer/); product UI/CLI help stays peer-name free.

## Operating Context

Used beside coding agents in git repos on macOS/Linux (WSL OK; Windows out of scope for CLI v1). Rituals: install binary, `memlayer install` for agent MCP/skills, `obs save` / `obs search` / `obs context`. Default listen is UDS; team TCP+TLS for self-hosted teams; Memlayer Cloud for managed SaaS. Docs live at https://memlayer.org; source at github.com/sanudatta11/memlayer.

## Capabilities and Constraints

- Public surface is an Astro Starlight docs site in `website/`, deployed to GitHub Pages with custom domain memlayer.org.
- Product is OSS (MIT OR Apache-2.0) for the self-host stack; Windows out of scope for the CLI v1.
- Site v1: no analytics; text wordmark (no custom logo mark yet); unversioned docs tracking main.
- Docs content tracks the README thesis; internal PRD stays out of public nav; public roadmap pointer is `docs/ROADMAP.md`.
- Open: do not invent LoCoMo “wins,” LangGraph-as-shipped, or claim Cloud regions/SLA before they exist — describe Cloud as the managed SaaS offering and point self-host at the shipped daemon.

## Brand Commitments

- Name: **memlayer** (lowercase wordmark).
- Voice: direct, technical, concrete — CLI/commands over hype.
- Binding references: coding-agent memory; **self-host + Cloud SaaS**; agent integrations in README and Integrations docs.
- User-approved design defaults: OSS framework-style docs (not heavy purple SaaS chrome); terminal/local-tool visual direction preferred for the self-host story.

## Evidence on Hand

- README and `website/src/content/docs/**` — real install and command copy for self-host.
- GitHub repo badges and license.
- LoCoMo methodology docs only until stratified disclosed scorecards exist — do not fabricate win claims or testimonials.
- Cloud SaaS is a stated offering; do not invent pricing, regions, or live signup UX until shipped.

## Product Principles

1. Dual deployment is the offer — self-host and Cloud are both intentional, not apologetic.
2. Docs serve evaluate *and* lookup — homepage persuades; interior pages prioritize scan and commands.
3. Show the mechanism (CLI/daemon/SQLite), don’t claim nebulous “AI memory.”
4. Stay maintainable inside Starlight — distinctive theme without abandoning docs chrome.
5. Prefer concrete commands and paths over abstract metaphors.

## Accessibility & Inclusion

Follow WCAG 2.2 AA contrast for body text; preserve keyboard navigation and Starlight’s built-in a11y. No product-specific alternate needs recorded yet.
