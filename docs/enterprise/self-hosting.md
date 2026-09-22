# Self-hosting statefulmemory-daemon

Runbook for running `statefulmemory-daemon` on infrastructure you control. This
covers deployment (Docker Compose, systemd, Kubernetes), backup/restore, and
multi-engineer ACL over TCP.

> See [cloud.md](cloud.md) for the difference between self-host and StatefulMemory
> Cloud, and [compliance.md](compliance.md) for what the daemon stores.

## Two transport modes

| Mode | When | Auth surface |
|---|---|---|
| **UDS** (`~/.statefulmemory/daemon.sock`) | Single user, one machine | Socket mode `0600`; the filesystem is the trust boundary. No bearer tokens. Admin RPCs are implicitly allowed for the owner. |
| **TCP + TLS** | Multi-engineer / team | `STATEFULMEMORY_LISTEN=tcp://HOST:PORT`; TLS required (rustls); bearer tokens in `tokens.db`; optional per-project grants. |

TCP mode is **mandatory TLS**: the daemon refuses to start TCP mode without
`STATEFULMEMORY_TLS_CERT` and `STATEFULMEMORY_TLS_KEY` pointing at a server cert + key.

## Building the daemon

**statefulmemory has no official registry image yet.** There is no published
OCI/Docker image and no `Dockerfile` in this repository. Plan to build the
binary from the workspace:

```bash
cargo build --release -p statefulmemory-cli
# → target/release/statefulmemory  (the CLI binary also embeds the daemon)
```

The binary is self-contained: `statefulmemory daemon start` runs the daemon in the
same process. `cargo install --path crates/statefulmemory-cli` also works if you
prefer an on-path install.

## Docker Compose

Minimal single-node run, UDS mode, data on a named volume. Compose builds
from the workspace checkout.

`docker-compose.yml`:

```yaml
services:
  statefulmemory:
    build:
      context: ../..              # repo root, where Cargo.toml lives
      dockerfile: infrastructure/Dockerfile.statefulmemory
    container_name: statefulmemory-daemon
    command: ["statefulmemory", "daemon", "start", "--foreground"]
    restart: unless-stopped
    environment:
      # Everything lives under this dir; volume-mount it for durability.
      STATEFULMEMORY_DATA_DIR: /data
      # Team TCP + TLS (single-host multi-shell is usually fine over UDS —
      # omit these for pure UDS mode):
      # STATEFULMEMORY_LISTEN: tcp://0.0.0.0:4432
      # STATEFULMEMORY_TLS_CERT: /certs/server.pem
      # STATEFULMEMORY_TLS_KEY: /certs/server-key.pem
    volumes:
      - statefulmemory-data:/data
      # Only needed in TCP+TLS mode. Generated with `statefulmemory team init-ca`.
      # - ./certs:/certs:ro
    healthcheck:
      test: ["CMD", "statefulmemory", "daemon", "status"]
      interval: 30s
      timeout: 5s
      retries: 3
    read_only: true
    tmpfs:
      - /tmp   # embedder / BGE model cache

volumes:
  statefulmemory-data:
```

An operator-provided Dockerfile at `infrastructure/Dockerfile.statefulmemory`
(relative to the repo). Minimal variant, using the generic Rust toolchain
image:

```dockerfile
FROM rust:1-bookworm AS build
WORKDIR /ws
COPY crates ./crates
COPY Cargo.toml Cargo.lock ./
COPY proto ./proto
RUN cargo build --release -p statefulmemory-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /ws/target/release/statefulmemory /usr/local/bin/statefulmemory
ENTRYPOINT ["statefulmemory"]
```

`read_only: true` is safe on the container filesystem: all writes go to
`STATEFULMEMORY_DATA_DIR` and `/tmp`. The BGE embedder may cache model weights; the
`tmpfs` on `/tmp` covers it. If you need a persistent model cache, mount a
second volume at the model directory instead.

## systemd unit

`/etc/systemd/system/statefulmemory.service`:

```ini
[Unit]
Description=statefulmemory daemon (persistent agent memory)
After=network.target

[Service]
Type=simple
User=appuser
Group=appgroup
WorkingDirectory=/srv/statefulmemory
ExecStart=/usr/local/bin/statefulmemory daemon start --foreground
Restart=on-failure
RestartSec=5
Environment=STATEFULMEMORY_DATA_DIR=/srv/statefulmemory/data
# TCP + TLS team mode:
# Environment=STATEFULMEMORY_LISTEN=tcp://0.0.0.0:4432
# Environment=STATEFULMEMORY_TLS_CERT=/etc/statefulmemory/server.pem
# Environment=STATEFULMEMORY_TLS_KEY=/etc/statefulmemory/server-key.pem
# Restrict sandbox (optional hardening):
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now statefulmemory
```

`--foreground` keeps the process attached so systemd owns lifecycle and
`Restart=` works. The daemon flushes an advisory lock (`daemon.lock`), so a
stray second instance fails fast instead of double-opening the SQLite files.

## Kubernetes (Deployment + Service)

Stateful-friendly Deployment with a PVC for `STATEFULMEMORY_DATA_DIR`. In TCP+TLS
mode the Service is the TLS endpoint; add your CA-signed or `team init-ca`
cert as a Secret.

```yaml
# statefulmemory-namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: statefulmemory
```

```yaml
# statefulmemory-stateful.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: statefulmemory-daemon
  namespace: statefulmemory
spec:
  replicas: 1          # single-writer: exactly one daemon owns the SQLite files
  selector:
    matchLabels:
      app: statefulmemory-daemon
  template:
    metadata:
      labels:
        app: statefulmemory-daemon
    spec:
      containers:
        - name: daemon
          image: registry.example.internal/statefulmemory:release  # you build this
          command: ["statefulmemory", "daemon", "start", "--foreground"]
          env:
            - name: STATEFULMEMORY_DATA_DIR
              value: /data
            # Team TCP + TLS mode — serve the private key from a Secret,
            # never baked into the image:
            # - name: STATEFULMEMORY_LISTEN
            #   value: tcp://0.0.0.0:4432
            # - name: STATEFULMEMORY_TLS_CERT
            #   value: /tls/server.pem
            # - name: STATEFULMEMORY_TLS_KEY
            #   valueFrom:
            #     secretKeyRef:
            #       name: statefulmemory-tls
            #       key: server-key.pem
          ports:
            - name: grpc
              containerPort: 4432
          volumeMounts:
            - name: data
              mountPath: /data
            - name: tls
              mountPath: /tls
              readOnly: true
            # embedder model cache
            - name: tmp
              mountPath: /tmp
          securityContext:
            readOnlyRootFilesystem: true   # all writes go to /data and /tmp
            allowPrivilegeEscalation: false
            capabilities:
              drop: ["ALL"]
          livenessProbe:
            exec:
              command: ["statefulmemory", "daemon", "status"]
            initialDelaySeconds: 15
            periodSeconds: 30
          readinessProbe:
            exec:
              command: ["statefulmemory", "daemon", "status"]
            initialDelaySeconds: 5
            periodSeconds: 10
          resources:
            requests:
              cpu: 250m
              memory: 512Mi
            limits:
              memory: 1Gi
      volumes:
        - name: tmp
          emptyDir: {}
        - name: tls
          secret:
            secretName: statefulmemory-tls
            optional: true
  volumeClaimTemplates:
    - metadata:
        name: data
      spec:
        accessModes: ["ReadWriteOnce"]
        resources:
          requests:
            storage: 10Gi
```

```yaml
# statefulmemory-service.yaml
apiVersion: v1
kind: Service
metadata:
  name: statefulmemory
  namespace: statefulmemory
spec:
  selector:
    app: statefulmemory-daemon
  ports:
    - port: 4432
      targetPort: grpc
      name: grpc
```

Notes:

- **replicas: 1 by design.** The per-project SQLite files are a single
  writer; run one daemon pod. Horizontal fan-out of clients (shells) is
  fine — they all speak to the one daemon.
- `readOnlyRootFilesystem: true` is supported: the daemon writes only under
  `STATEFULMEMORY_DATA_DIR`, `/tmp`, and stdout logs. If the BGE model cache must
  persist across restarts, mount the `emptyDir` at the model dir and point
  `STATEFULMEMORY_BGE_MODEL_DIR` at it.
- PVC via `volumeClaimTemplates` gives each pod its own volume; with one
  replica it round-trips to the same claim.
- `livenessProbe`/`readinessProbe` shell out to `statefulmemory daemon status`,
  which fails fast (non-zero) when the socket is missing.

## Backup and restore

Backup unit of work is a **project**. The portable format is a compressed
`.mem` archive; since **v2 it carries the observation set, sessions, prompts,
facts, and the entity graph** (`entities`, `entity_edges`).

Daily cron (one archive per project — add one line per project):

```cron
# crontab -e — export each project at 02:00
0 2 * * *  statefulmemory mem export --out /var/backups/statefulmemory/myapp-$(date +\%F).mem --project myapp
0 3 * * *  statefulmemory mem export --out /var/backups/statefulmemory/tooling-$(date +\%F).mem --project tooling
```

For many projects, drive the loop off `statefulmemory project list --output json`
(export is an RPC; run the loop on the same host as the daemon):

```bash
statefulmemory project list --output json | jq -rc '.[].name' | while IFS= read -r p; do
  statefulmemory mem export --out "/var/backups/statefulmemory/$p-$(date +%F).mem" --project "$p"
done
```

Encrypt the archive (BYOK — same phrase or seed file unlocks on import):

```bash
statefulmemory mem export --out backup.mem --seed-file ./phrase.txt
```

Restore:

```bash
# merge (default): upsert by sync_id — safe incremental restore
statefulmemory mem import backup.mem --mode merge

# replace: wipe project first, then insert
statefulmemory mem import backup.mem --mode replace
```

`--mode replace` is destructive for the target project. `merge` is idempotent
across re-runs.

Operational guidance:

- Archive while the daemon is up; export/import are RPCs to the daemon.
- Never back up only the SQLite files while the daemon is writing; the `.mem`
  archive is the consistent, migration-safe artifact. A raw file copy of
  `projects/*.db` + WAL is a crash-dangerous second option, not a substitute.
- The seed phrase (`--seed-phrase` / `--seed-file`) is **not** stored in the
  archive or on disk by statefulmemory. Lose it → archive is unrecoverable. Prefer
  `--seed-file` over `--seed-phrase` because argv is visible to `ps`;
  but assume both leak unless you lock down /proc.

## Multi-engineer ACL (TCP mode)

Team mode is a three-step setup. All `token-*` and `grant-*` verbs are
admin RPCs. Over **UDS the admin guard is implicit** (socket `0600`); over
**TCP you must present an admin bearer token**.

1. Generate the CA + server leaf (run once on the box that hosts the
   daemon; local only, no daemon RPC):

```bash
statefulmemory team init-ca ./certs          # -> ca.pem, server.pem, server-key.pem
```

2. Start the daemon in TCP mode with the certs:

```bash
STATEFULMEMORY_LISTEN=tcp://0.0.0.0:4432 \
STATEFULMEMORY_TLS_CERT=./certs/server.pem  \
STATEFULMEMORY_TLS_KEY=./certs/server-key.pem \
statefulmemory daemon start --foreground
```

3. Mint a bootstrap admin token (over UDS, admin is implicit):

```bash
statefulmemory team token-create --name ops --admin
# prints the 64-hex token exactly once — record it now
```

Then, as that admin:

```bash
# per-engineer tokens
statefulmemory team token-create --name alice
statefulmemory team token-create --name bob --admin        # reserved for ops

# inspect / revoke
statefulmemory team token-list
statefulmemory team token-revoke alice

# per-project grants (read | write)
statefulmemory team grant --project webapp --principal alice --role read
statefulmemory team grant --project webapp --principal alice --role write
statefulmemory team grant-list
statefulmemory team grant-revoke --project webapp --principal alice
```

> Honest scope note: `token-*` / `grant-*` are daemon RPCs. The CLI shell
> examples above work wherever the daemon is reachable; the client
> library (`statefulmemory-client`) is what speaks TLS + bearer tokens over TCP,
> and today the CLI's own connection path is UDS (`connect_uds`). The
> gRPC ACL surface (admin guard, token store, grants) is implemented and
> integration-tested; a first-class `--host/--token` TCP client flag on the
> CLI is not yet exposed. Audit before quoting "CLI over TCP" to a security
> reviewer.

Grant semantics:

- Grants are **opt-in**. A project with no grants stays open to any valid
  token; gating is inherited from the moment the first grant exists
  (then a principal with no grant is denied, and "read" rejects writes).
- Admin token gates `Shutdown`, `token-*` RPCs, and
  `project delete --hard`.
- Client side: CLI connects with the server cert's CA (`ca.pem`) and the
  holder's bearer token; daemon validates token against `tokens.db`.

### Transport security summary

| Surface | Control |
|---|---|
| Unix socket | `chmod 0600` enforced by daemon at bind. UDS = single-user trust boundary. |
| TCP wire | rustls (TLS 1.2/1.3) mandatory; private key `server-key.pem` must be mounted read-only. |
| AuthZ | Bearer tokens (`tokens.db`); admin flag gates admin RPCs. |
| At-rest | SQLite files plain on disk. Encrypt volumes or the `.mem` archive (`--seed-*`). No built-in DB-level encryption. |