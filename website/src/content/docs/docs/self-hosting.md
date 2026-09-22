---
title: Self-hosting and Cloud
description: Self-host on UDS or team TCP+TLS, or use StatefulMemory Cloud SaaS for managed hosting.
---

StatefulMemory ships as **open-source self-host** and as **StatefulMemory Cloud** (managed
SaaS). Same coding-agent memory product; you choose who runs the daemon.

## Self-host: per-user UDS

On first CLI or MCP use, the daemon auto-starts and listens on a Unix domain
socket under `~/.statefulmemory/` (filesystem permissions are the trust boundary).
macOS and Linux are supported; Windows is not for the CLI v1.

```bash
statefulmemory daemon start --foreground   # optional; usually auto-spawned
statefulmemory doctor
```

Project databases live under `~/.statefulmemory/`. You can copy, backup, or delete
them like any other SQLite files. Hybrid search/context runs on that machine
without a StatefulMemory Cloud account.

## Self-host: team TCP + TLS

For a shared daemon on a network **you** operate, bind TCP with mandatory TLS
and bearer tokens:

```bash
# Generate a self-signed CA + leaf cert
statefulmemory team init-ca ./certs

# Start daemon (example env — paths and listen addr are yours)
export STATEFULMEMORY_LISTEN=tcp://0.0.0.0:50051
export STATEFULMEMORY_TLS_CERT=./certs/server.pem
export STATEFULMEMORY_TLS_KEY=./certs/server-key.pem
statefulmemory daemon start --foreground
```

Clients trust the CA via `STATEFULMEMORY_TLS_CA`. Admin commands (token create /
revoke, hard project delete, remote daemon stop) require an admin bearer
token. See `statefulmemory team --help`.

TCP without both cert and key refuses to start.

## Cloud SaaS {#cloud-saas}

**StatefulMemory Cloud** is the managed SaaS offering: persistent agent memory with
hosting, upgrades, and multi-seat ops handled for you. Clients still use the
CLI / MCP / install targets where applicable; the control plane and storage
run as a hosted service.

Use Cloud when you want managed ops. Use self-host when you want data and
process on your own machines. Both are intentional product paths — not
either/or ideology.

Signup and regional details will be published on [statefulmemory.dev](https://statefulmemory.dev)
as the service rolls out. Until then, install and run the open-source daemon
from [Install](/docs/install/).

## Related

- [Architecture](/docs/architecture/)
- [Why StatefulMemory?](/docs/why-statefulmemory/)
- [Integrations](/docs/integrations/)
