---
title: Enterprise
description: Self-host memlayer for a team — Docker, systemd, Kubernetes, per-project grants, backup/restore, and the compliance evidence pack.
---

For teams and enterprises that want to **own their memory system**, memlayer
self-hosts on your infra: a single daemon → per-project SQLite, git-tied
anchors, and portable `.mem` archives. There is no vendor round-trip on the
retrieval path.

## Deploy

The production surface is the same `memlayer-daemon` the CLI auto-spawns. Run
it in the foreground on a host, point clients at it over TCP+TLS, and attach
per-project grants.

```bash
# one machine, systemd
memlayer daemon start --foreground
```

- **Docker Compose**, **systemd**, and **Kubernetes** manifests live in
  `docs/enterprise/self-hosting.md` (repo).
- TCP mode: `memlayer team init-ca <dir>` → mint tokens with
  `memlayer team token-create --name ci --admin`.
- **Per-project ACLs**: `memlayer team grant --project <p> --principal <token>
  --role read|write` gates the write handlers; admin tokens bypass.

## Backup / restore

```bash
0 2 * * * cd /srv/memlayer && memlayer mem export --out backups/$(date +\%F).mem
```

`.mem` v2 archives carry the entity graph; restore with `memlayer mem import`.

## Compliance evidence

Honest posture: memlayer is local-first. The compliance pack (`docs/enterprise/
compliance.md`) documents the data map, retention, deletion semantics, and the
audit trail in `queries.log` — and explicitly does **not** claim certs that
haven't been earned (no SOC 2/HIPAA assertions without evidence).

## Cloud

A managed **Memlayer Cloud** is on the roadmap: signup waitlist + regions TBD.
Everything below the line (graph, portability, grants, UI) is self-host
today. See `docs/enterprise/cloud.md` for the honest status.