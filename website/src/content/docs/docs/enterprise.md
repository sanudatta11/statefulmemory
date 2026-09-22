---
title: Enterprise
description: Self-host statefulmemory for a team — Docker, systemd, Kubernetes, per-project grants, backup/restore, and the compliance evidence pack.
---

For teams and enterprises that want to **own their memory system**, statefulmemory
self-hosts on your infra: a single daemon → per-project SQLite, git-tied
anchors, and portable `.mem` archives. There is no vendor round-trip on the
retrieval path.

## Deploy

The production surface is the same `statefulmemory-daemon` the CLI auto-spawns. Run
it in the foreground on a host, point clients at it over TCP+TLS, and attach
per-project grants.

```bash
# one machine, systemd
statefulmemory daemon start --foreground
```

- **Docker Compose**, **systemd**, and **Kubernetes** manifests live in
  `docs/enterprise/self-hosting.md` (repo).
- TCP mode: `statefulmemory team init-ca <dir>` → mint tokens with
  `statefulmemory team token-create --name ci --admin`.
- **Per-project ACLs**: `statefulmemory team grant --project <p> --principal <token>
  --role read|write` gates the write handlers; admin tokens bypass.

## Backup / restore

```bash
0 2 * * * cd /srv/statefulmemory && statefulmemory mem export --out backups/$(date +\%F).mem
```

`.mem` v2 archives carry the entity graph; restore with `statefulmemory mem import`.

## Compliance evidence

Honest posture: statefulmemory is local-first. The compliance pack (`docs/enterprise/
compliance.md`) documents the data map, retention, deletion semantics, and the
audit trail in `queries.log` — and explicitly does **not** claim certs that
haven't been earned (no SOC 2/HIPAA assertions without evidence).

## Cloud

A managed **StatefulMemory Cloud** is on the roadmap: signup waitlist + regions TBD.
Everything below the line (graph, portability, grants, UI) is self-host
today. See `docs/enterprise/cloud.md` for the honest status.