# memlayer Enterprise docs

Operational and procurement material for running memlayer on infrastructure
you control.

| Doc | What it covers |
|---|---|
| [self-hosting.md](self-hosting.md) | Deployment runbooks: Docker Compose, systemd unit, Kubernetes (Deployment + PVC + read-only root FS), backup/restore via `.mem` archives, and multi-engineer ACL over TCP (`team init-ca` / `token-*` / `grant-*`). |
| [compliance.md](compliance.md) | Data map (`~/.memlayer/**`), retention + deletion, audit trail (`queries.log`), compliance posture statement (local-first; **no certifications claimed**), enterprise security surface. |
| [cloud.md](cloud.md) | Honest status of Memlayer Cloud: waitlist + regions TBD, no invented SLAs; what self-host covers today vs. what Cloud would add. |

## Quick orientation

- **Product-level architecture:** see [`AGENTS.md`](../../AGENTS.md) — the
  daemon→SQLite/FTS5 layout, worker pools (embed/extract/verify), config
  precedence, and CLI command quick-reference that this repo's docs assume.
- **Roadmap:** [`../ROADMAP.md`](../ROADMAP.md) for what is shipped vs. open;
  retrieval pipeline plans in [`../RETRIEVAL_ROADMAP.md`](../RETRIEVAL_ROADMAP.md).
- **Specs:** [`../specs/`](../specs/) for design documents (e.g. code-anchored
  memory).

## Rules of thumb used in these docs

- No certification claims without evidence. If you see "SOC 2 ready" or
  similar anywhere here, it is a mistake — open an issue.
- All facts are stated against the current codebase (CLI verbs, schema,
  env vars). Recheck with `memlayer team --help`, `memlayer daemon --help`,
  and the crate source before quoting these docs in a security review.
- `memlayer daemon start --foreground` is the foreground form used by the
  systemd / container / k8s examples; auto-spawn launches the daemon itself
  and `--foreground` is only for supervised lifecycles.