# statefulmemory compliance pack

Honest, evidence-based documentation for procurement, security-review, and
privacy questions. No certifications are claimed — see the posture statement
below for what is (and is not) being asserted.

## Data map

By default everything lives under `~/.statefulmemory/` (override with
`STATEFULMEMORY_DATA_DIR`). Contents:

| Path | What it holds | Notes |
|---|---|---|
| `daemon.sock` | Unix domain socket (mode `0600`) | Transport endpoint, single-user mode. |
| `daemon.lock` / `daemon.pid` | Advisory flock + PID for spawn races | Prevents two daemons opening the same SQLite files. |
| `daemon.log` | Daemon tracing log, JSON lines | Rotated: **10 MB × 5 files** by the tracing-appender. |
| `global.sqlite` (+ `-wal`/`-shm`) | Cross-project mirror; powers `--all-projects` | Mirrors saves across projects (BM25 index). |
| `projects/<name>.db` | Per-project SQLite + FTS5 (+ optional sqlite-vec dense) | The primary store for a project's observations. |
| `projects/<name>.config.toml` | Per-project config override | Env > project > global > code defaults. |
| `config.toml` / `config.json` | Global config | Written by `statefulmemory install` and `statefulmemory config`. |
| `tokens.db` | Bearer-token store (TCP mode) | Token names plus admin flag, stored so TCP connections can be validated. |
| `queries.log` | JSONL audit trail of CLI calls | See below. |
| `diagnostics-<ts>.json` | Diagnostics dump on SIGUSR1 | Debug-only. |

Per-project SQLite files hold observations (type / title / content / code
anchors / superseded_by), FTS entries, embeddings, sessions, and (schema ≥ V11)
the entity graph (`entities` / `entity_mentions` / `entity_edges`).

## Audit trail

`queries.log` is a **fail-silent JSONL append log** recorded by the CLI for
`obs` / `session` / `hook` verbs. Each row: timestamp, command, project,
result count, duration. Set `STATEFULMEMORY_AUDIT_FULL=1` to additionally carry the
query string and the top returned observations (`{id, type, title}`, each
field truncated to 4 KB). The recorder never blocks a save: read-only log
path, full disk, or serialization errors drop the entry silently.

- `STATEFULMEMORY_AUDIT_FULL=1` is **opt-in**; default rows avoid leaking query
  contents and return sets.
- **No built-in rotation or retention window** for `queries.log`. It is plain
  appending; rotate with logrotate / systemd journal rotation / container
  logging as you would any other log.
- `daemon.log` self-rotates (10 MB × 5).

Audit is for retrieval debugging, not a formal SOC 2 evidence stream.

## Retention and deletion

Retention defaults:

- Observations have **no TTL**. Nothing expires automatically. Older
  observations are demoted / superseded (soft-delete by `superseded_by_id`
  on matching saves), but rows remain until deleted.
- Embeddings / indexes are rebuilt on save / `statefulmemory reindex`; no periodic
  GC job runs by default.
- Logs rotate as described above only.

Deletion commands (all explicit — statefulmemory never silently GCs):

| Command | Effect |
|---|---|
| `obs delete <id>` | Soft-delete (marks superseded / inactive). |
| `obs delete <id> --hard` | Deletes the row + FTS entry. |
| `project delete <name>` | Deletes a project's data. |
| `project delete <name> --hard` | Removes the DB file. Admin-only over TCP. |
| `mem import F.mem --mode replace` | Wipes the target project, then loads the archive. |
| `statefulmemory clean` | **Wipes all stored observations and stops the daemon.** |
| `statefulmemory project prune` | Lists projects with zero active observations (dry-run flag for review). |

`.mem` export content: compressed archive of observations, sessions, prompts,
facts, and (v2+) the entity graph. Deleting the source does not retroactively
delete an exported `.mem` — treat archives as PII copies.

## Compliance posture statement

- **Local-first.** Default deployment stores all memory on the machine that
  runs the daemon (`~/.statefulmemory/`). No telemetry upload, no account, no cloud
  dependency for retrieval.
- **No compliance certifications are claimed.** statefulmemory has not submitted to
  SOC 2, HIPAA, ISO 27001, or FedRAMP. Do not state otherwise in marketing or
  procurement responses. This is a project status statement, not a substitute
  for a vendor attestation.
- **Third-party data flow (LLM steps only, opt-in).** When `extract`,
  `conflict`, `search.rerank`, or `decide` are enabled and an agent CLI is on
  `PATH`, statefulmemory pipes prompt text to that CLI (shelled out, stdin/argv).
  Model selection is the agent CLI's (e.g. a hosted LLM). These features are
  **off by default** (`extract.enabled=false`, `conflict.enabled=true` after
  `statefulmemory install`, `rerank=false`). Hybrid search itself (BM25 + local
  BGE-small embeddings) is fully local.
- **At-rest encryption.** SQLite files are plaintext. Two encryption options:
  1. Volume-level (LUKS / cloud disk encryption) for
     the data dir.
  2. BYOK on export: `mem export --seed-phrase` / `--seed-file` encrypts
     the `.mem` archive (seed-phrase / seed-file required on import). The
     seed is never stored by statefulmemory — losing it loses the archive.
- **Key management is operator-owned.** No built-in KMS integration, no key
  escrow. BYOK = you hold the seed; rotate and store it out-of-band.

## Enterprise security surface

| | |
|---|---|
| Unix socket | `daemon.sock` is bound mode `0600`. Filesystem permissions are the auth boundary in single-user mode. |
| TCP transport | TLS mandatory (`STATEFULMEMORY_LISTEN=tcp://…` aborts startup without `STATEFULMEMORY_TLS_CERT` / `STATEFULMEMORY_TLS_KEY`). rustls (TLS 1.2/1.3). |
| Bearer tokens | `tokens.db` holds token names + admin flag. Admin gates: `Shutdown`, `token-*`, `project delete --hard`. |
| Per-project grants | Managed via `statefulmemory team grant` — principal → role (`read`/`write`) per project. Opt-in: projects with no grants are open to any valid token; once gated, nongranted principals are denied and `read` rejects writes. |
| Client validation | Client library pins the daemon's `ca.pem` (from `team init-ca`) and presents the bearer token over TLS. **Scope note:** the daemon side (TLS, token store, admin guard, grants) is implemented; today the CLI's own transport is UDS, TCP access rides the `statefulmemory-client` library surface. |
| Auditable / integrity | `statefulmemory doctor` runs DB integrity audit; `statefulmemory verify` re-checks code anchors vs git history (wired to post-commit / post-merge / post-checkout hooks). |

## Map to common questionnaire items

| Question | Answer |
|---|---|
| Where is data stored? | On the daemon host under `~/.statefulmemory/` (or `STATEFULMEMORY_DATA_DIR`). |
| Is it encrypted at rest? | No built-in DB encryption; volume encryption or seed-protected `.mem` archives. |
| Does data leave the host? | Only for opt-in LLM steps when you wire a hosted agent CLI. |
| Is it SOC 2 / HIPAA certified? | No. No compliance certifications are held or claimed. See posture statement. |
| Can we delete our data? | Yes — `obs delete --hard`, `project delete --hard`, or `statefulmemory clean`. No retention locks. |
| Is there an audit log? | `queries.log` JSONL (fail-silent). |
| Certifications | None claimed in this documentation set. |