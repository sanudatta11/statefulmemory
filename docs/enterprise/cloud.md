# StatefulMemory Cloud — status and positioning

Honest status: **StatefulMemory Cloud is a stated product path, not a shipping
service.** There is no SLA, no region list, and no public endpoint to point a
client at today. This page says so explicitly so nobody misreads the docs.

## Status

- **Waitlist / signup:** rolling out on statefulmemory.dev; signup indicates
  interest. No backfill of capacity, regions, or pricing is committed here.
- **Regions: TBD.** No region guarantees, no residency commitment, no data
  retention promise for Cloud — those terms will be decided when the managed
  service ships.
- **SLAs: none.** statefulmemory makes no uptime, RPO, or RTO commitment for Cloud.
  Treat any SaaS-style claims elsewhere in this repo as roadmap intent, not
  contract.

If a procurement conversation requires contractual SLA / DPA language, the
answer today is: self-host, and rely on your own operational runbook
(see [self-hosting.md](self-hosting.md)).

## What self-host covers today

Everything in this repo is self-hostable and is the primary path:

- Single-user mode via Unix socket (`~/.statefulmemory/daemon.sock`, mode `0600`).
- Team mode over TCP + TLS with bearer tokens and per-project grants
  (`statefulmemory team init-ca` / `token-create` / `grant`, etc.).
- Per-project SQLite + FTS5 with an optional BGE-small dense index; hybrid
  search is fully local — no external search API key on self-host.
- Backup/restore via `statefulmemory mem export` / `mem import` (`.mem` v2 carries
  the entity graph).
- Optional LLM steps (`extract`, `conflict`, `rerank`, `decide`) shell out to
  whatever agent CLI is on `PATH` — including local/open-source models.
  Nothing in self-host requires StatefulMemory Cloud.

## What Cloud would add (roadmap, not description)

When the managed service ships, the delta is expected to be:

- **Managed daemon** — hosting, TLS, certificates, and upgrades handled for
  you; same thin CLI / MCP client surface.
- **Team dashboards** — usage, search inspection, member and grant
  management in a web UI rather than a CLI.
- Everything else stays identical: same storage model, same client protocol,
  same `.mem` format.

Cloud is additive, not a replacement. Self-host remains first-class and open
source; the Cloud SaaS shares the same product thesis (coding-agent memory
via MCP/CLI clients) with hosting done by us.

## Decision guide

| Need | Path |
|---|---|
| Single engineer, one machine | Self-host, UDS mode (out of the box). |
| Small team, shared daemon | Self-host team daemon (TCP + TLS + grant surface; see `docs/enterprise/self-hosting.md` — note the CLI currently connects over UDS, TCP access rides the client library). |
| Contractual SLA / DPA / residency | Not available in Cloud yet; self-host under your own ops. |
| "Just works, managed" | Watch statefulmemory.dev for Cloud waitlist; nothing to configure today. |