---
title: Web UI
description: Browse memory, trace the entity graph, and run Decide from a loopback dashboard — `memlayer ui`.
---

`memlayer ui` serves a lightweight single-page dashboard on your loopback
interface. It talks to the daemon over the same UDS gRPC channel the CLI uses,
so **no extra server, no tokens, no network exposure**.

```bash
memlayer ui
# memlayer UI → http://127.0.0.1:4687     hit Ctrl-C to stop
```

## Security model

- Binds **127.0.0.1 only**; the CLI refuses any other host.
- No auth token — the listener never leaves the machine, matching the daemon's
  UDS `0600` posture.
- For LAN/team exposure, put a TLS reverse proxy (Caddy/nginx) in front; the
  underlying gRPC RPCs still carry bearer-token auth when served over TCP.

## Dashboard

- **Stats** — observations scanned + consolidation candidates (Dream-lite).
- **Search** — hybrid search over the project (mode `hybrid`, top-15).
- **Graph** — enter an entity, get a radial force-style render of the
  neighborhood (hops ≤ 2) with weighted typed edges.
- **Decide** — ask a question; the daemon runs the conflict judge + synthesis
  via your agent CLI.

## Endpoints

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/` | — | dashboard HTML |
| POST | `/api/search` | `{"query": "…"}` | observation hits |
| POST | `/api/graph` | `{"entity": "…", "hops": 1}` | entities + edges |
| POST | `/api/stats` | `{}` | dream-scan counts |
| POST | `/api/decide` | `{"question": "…"}` | recommendation |

The page is a single self-contained HTML file embedded into the binary — no
Node/build pipeline, no CDN.