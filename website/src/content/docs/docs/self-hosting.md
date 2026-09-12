---
title: Self-hosting and Cloud
description: Self-host on UDS or team TCP+TLS, or use Memlayer Cloud SaaS for managed hosting.
---

Memlayer ships as **open-source self-host** and as **Memlayer Cloud** (managed
SaaS). Same coding-agent memory product; you choose who runs the daemon.

## Self-host: per-user UDS

On first CLI or MCP use, the daemon auto-starts and listens on a Unix domain
socket under `~/.memlayer/` (filesystem permissions are the trust boundary).
macOS and Linux are supported; Windows is not for the CLI v1.

```bash
memlayer daemon start --foreground   # optional; usually auto-spawned
memlayer doctor
```

Project databases live under `~/.memlayer/`. You can copy, backup, or delete
them like any other SQLite files. Hybrid search/context runs on that machine
without a Memlayer Cloud account.

## Self-host: team TCP + TLS

For a shared daemon on a network **you** operate, bind TCP with mandatory TLS
and bearer tokens:

```bash
# Generate a self-signed CA + leaf cert
memlayer team init-ca ./certs

# Start daemon (example env — paths and listen addr are yours)
export MEMLAYER_LISTEN=tcp://0.0.0.0:50051
export MEMLAYER_TLS_CERT=./certs/server.pem
export MEMLAYER_TLS_KEY=./certs/server-key.pem
memlayer daemon start --foreground
```

Clients trust the CA via `MEMLAYER_TLS_CA`. Admin commands (token create /
revoke, hard project delete, remote daemon stop) require an admin bearer
token. See `memlayer team --help`.

TCP without both cert and key refuses to start.

## Cloud SaaS {#cloud-saas}

**Memlayer Cloud** is the managed SaaS offering: persistent agent memory with
hosting, upgrades, and multi-seat ops handled for you. Clients still use the
CLI / MCP / install targets where applicable; the control plane and storage
run as a hosted service.

Use Cloud when you want managed ops. Use self-host when you want data and
process on your own machines. Both are intentional product paths — not
either/or ideology.

Signup and regional details will be published on [memlayer.org](https://memlayer.org)
as the service rolls out. Until then, install and run the open-source daemon
from [Install](/docs/install/).

## Related

- [Architecture](/docs/architecture/)
- [Why Memlayer?](/docs/why-memlayer/)
- [Integrations](/docs/integrations/)
