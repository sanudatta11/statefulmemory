# Memlayer — Product Requirements Document

A persistent-memory service for AI coding agents. Greenfield Rust implementation.

This PRD is a working spec for a Rust implementation. All defaults, names, and shapes below are normative unless explicitly marked open.

---

## 0. Project Identity

- **Name**: `memlayer`
- **Binary name**: `memlayer`
- **Data dir**: `~/.memlayer/`
- **Env var prefix**: `MEMLAYER_*`
- **Implementation language**: Rust (Go optional for a future subsystem only if a Rust solution is unfit)
- **Target platforms**: `linux/amd64`, `linux/arm64`, `darwin/amd64`, `darwin/arm64`. Windows out of scope for v1.
- **License**: MIT AND Apache-2.0 (dual-license; users choose either).

### Design Principles
- Per-project memory store with FTS5 full-text search.
- Topic-key upserts for evolving knowledge.
- Sessions, observations, prompts as the core entities.
- Git-based sync via per-repo `.memlayer/` directory.
- Minimum-friction agent integration through prompt-level skills/rules.
- **CLI + MCP.** Agents talk to the daemon over UDS gRPC (`memlayer` CLI) or
  stdio MCP (`memlayer mcp` / `memory_*` tools). No HTTP REST.
- **Daemon-backed.** A single user-local daemon owns SQLite; CLI talks to it over gRPC on a Unix socket.
- **Per-project DB files** (one SQLite file per project), not a single global DB.
- Hybrid retrieval (BM25 + optional dense embeddings via BGE-small / sqlite-vec).
- 1000-concurrent-agent acceptance bar, batched-commit write path, simplified doctor with `--auto-repair`, prompt-level setup integrations for multiple agents, scoped observations (`project` / `personal` / `team`), read-only mode on disk-full.

---

## 1. Goals & Non-Goals

### Goals
1. **Throughput under write-heavy load.** 1000 simultaneous agents writing memories with no errors.
2. **Low latency.** `obs save` p95 < 10ms warm; `obs search` p95 < 100ms over 1M rows.
3. **Single static binary.** No runtime, no system SQLite dep, no C++ deps.
4. **Ease of use.** Zero-config first run. Auto-spawned daemon. Agents discover memlayer through a single prompt-level skill file.
5. **Round-trippable export.** Markdown export and import are symmetric.
6. **Team-hostable.** A daemon can be exposed over TCP for an entire team.

### Non-goals
- **No vector search / embeddings** in v1.
- **No multi-region replication.** Git sync is the only cross-machine sharing mechanism.
- **No write throughput beyond ~5K saves/sec aggregate** in v1. Escape hatch is per-project sharding (already designed); RocksDB migration is a v2 escape route.
- **No Windows support** in v1.
- **No telemetry, OTLP, or Prometheus** in v1. These are all deliberately excluded — not just deferred.

---

## 2. Architecture

```
┌────────────────────────── User host (or team server) ──────────────────────────┐
│                                                                                │
│  agent ─┐                                                                      │
│         ├──▶ memlayer (CLI)  ──gRPC over UDS──▶  memlayer daemon                │
│  agent ─┘                                            │                          │
│                                                      ├── tokio runtime          │
│                                                      ├── tonic gRPC server      │
│                                                      ├── write thread (mpsc)    │
│                                                      ├── per-project conn LRU   │
│                                                      ├── tracing JSON logs      │
│                                                      └── git-sync engine        │
│                                                                                │
│  ~/.memlayer/                                                                  │
│    daemon.sock         ← Unix domain socket, mode 0600                          │
│    daemon.lock         ← spawn lock                                             │
│    daemon.pid                                                                   │
│    daemon.log          ← tracing JSON, rotated 10MB×5                           │
│    projects/<name>.db  ← per-project SQLite + FTS5 (one file per project)       │
│                                                                                │
│  <repo>/.memlayer/                                                             │
│    manifest.json                                                               │
│    chunks/<id>.jsonl.zst                                                       │
│    config.json (optional, sets project name)                                   │
└────────────────────────────────────────────────────────────────────────────────┘
```

### Process model
- **CLI** is a thin gRPC client. Resolves the daemon socket, makes one RPC, prints the response, exits.
- **Daemon** is long-lived. Auto-spawned on first CLI call. Owns all DB connections.
- **Tokio runtime** runs the gRPC server and any background tasks (sync, log rotation).
- **Write thread** is a single dedicated `std::thread` running blocking `rusqlite` calls. Tokio tasks send writes via `tokio::sync::mpsc`. Rationale: avoids async-SQLite wrappers; preserves SQLite's single-writer-per-DB invariant explicitly.
- **Per-project connection cache** is an LRU keyed by project name (capacity 256). Each entry holds an open `rusqlite::Connection` (read pool) and a write-channel sender for that project. Eviction closes the connection.

### Crate choices (locked)
| Concern | Crate |
|---|---|
| SQLite + FTS5 | `rusqlite` with `features = ["bundled-full"]` |
| Async runtime | `tokio` |
| gRPC | `tonic` + `prost` |
| CLI parsing | `clap` (derive) + `clap_complete` |
| Logging | `tracing`, `tracing-subscriber` (JSON formatter), `tracing-appender` (rotation) |
| Config | `serde`, `serde_json`, optionally `toml` |
| Compression | `zstd` |
| TUI | `ratatui` + `crossterm` |
| Errors | `thiserror`, `anyhow` (CLI only) |
| UUIDs | `uuid` v4 |
| Hashing | `sha2` |
| Migrations | `refinery` (embedded SQL, version tracked in `schema_meta`) |
| TLS (TCP mode) | `rcgen` (self-signed CA + leaf certs) |

No CGO, no external system libs. Verified single-binary distribution.

---

## 3. CLI Surface

### 3.1 Shape
Noun-first, kubectl-style. Top-level groups, each with verbs.

```
memlayer <group> <verb> [args] [flags]
```

**Groups**: `obs`, `session`, `prompt`, `project`, `sync`, `daemon`, `service`, `setup`, `tui`, `doctor`, `logs`, `team`, `version`, `completions`.

### 3.2 Output
- **TTY-aware default**: stdout is a terminal → human-readable text/table. Stdout is piped/redirected → JSON.
- **Override**: `--output {text,json,yaml}` on every command.
- **Detection**: `IsTerminal::is_terminal(&io::stdout())` (Rust stdlib ≥1.70).
- **Color**: respects `NO_COLOR` env var and `--no-color` flag.

### 3.3 Subcommand inventory

| Group / verb | Args / flags | Behavior |
|---|---|---|
| `obs save` | `--title T (req) --content C (req) [--type T] [--scope {project,personal,team}] [--topic K] [--project P] [--session S]` | Save observation. Scope defaults to `project`. Returns `id, sync_id, similar_observations:[…]`. |
| `obs update` | `<id> [--title] [--content] [--type] [--scope] [--topic]` | Update by id. |
| `obs delete` | `<id> [--hard]` | Soft-delete by default. |
| `obs get` | `<id>` | Full row. |
| `obs search` | `<query> [--type T] [--project P] [--scope S] [--limit N (default 10, max 50)] [--all-projects]` | FTS5 BM25 search. `--all-projects` fans out via `ATTACH` across all project DBs (capped at 32). |
| `obs recent` | `[--project P] [--scope S] [--limit N] [--cursor C]` | Recent observations, paginated. |
| `obs list` | `[--project P] [--scope S] [--limit N] [--sort created_at:desc] [--due-for-review] [--cursor C]` | List with filters, paginated. |
| `obs context` | `[--project P] [--scope S]` | Markdown context summary for prompt injection. Returns context inline. |
| `obs timeline` | `<id> [--before N] [--after N]` | Chronological neighbors. |
| `obs suggest-topic-key` | `[--type T] [--title S] [--content C]` | Suggest a stable topic key. |
| `obs capture-passive` | `[--session S] [--text -]` | Extract `## Key Learnings:` from stdin. |
| `session start` | `<id> [--directory D]` | Idempotent session start. Returns session object + inline context summary (same as `obs context`). |
| `session end` | `<id> [--summary S]` | Mark session complete. |
| `session summary` | `<session_id> --content C` | End-of-session structured summary. |
| `session list` | `[--project P] [--limit N] [--cursor C]` | Recent sessions, paginated. |
| `session get` | `<id>` | Session detail. |
| `session delete` | `<id>` | Fails if observations reference it. |
| `prompt save` | `<content> [--session S] [--project P]` | Save user prompt. |
| `prompt search` | `<query> [--project P] [--limit N]` | FTS5 search across prompts. |
| `prompt recent` | `[--project P] [--limit N] [--cursor C]` | Recent prompts, paginated. |
| `prompt delete` | `<id>` | Permanent (no soft-delete for prompts). |
| `project list` | `[--cursor C]` | All projects with counts, paginated. |
| `project current` | — | Detect from cwd. |
| `project merge` | `--from N1[,N2,…] --to N` | Merge variants into canonical. |
| `project delete` | `<name> [--hard]` | Cascade delete. |
| `project consolidate` | `[--all] [--dry-run]` | Interactive merge of similar names. |
| `project prune` | `[--dry-run]` | Remove projects with 0 observations. |
| `sync export` | `[--all] [--project P]` | Write a chunk to `<repo>/.memlayer/chunks/`. |
| `sync import` | — | Apply unimported chunks from `<repo>/.memlayer/`. |
| `sync status` | `[--project P]` | Lifecycle, last-error, deferred counts. |
| `sync export-json` | `[file]` | Single JSON dump (default `memlayer-export.json`). |
| `sync import-json` | `<file>` | Idempotent import on `sync_id`. |
| `sync export-md` | `<dir> [--project P] [--since DATE]` | Round-trip markdown export. |
| `sync import-md` | `<dir>` | Symmetric markdown import. |
| `daemon start` | `[--foreground]` | Start daemon. |
| `daemon stop` | — | Graceful shutdown. |
| `daemon status` | — | Uptime, active connections, project DB counts. |
| `daemon restart` | — | Stop + start. |
| `service install` | `[--user]` | Write launchd plist (macOS) or systemd user unit (Linux). |
| `service uninstall` | — | Remove unit. |
| `setup` | `[agent] [--scope global\|project] [--print]` | Write prompt-level skill/rule files. Detection picker if no agent. |
| `tui` | — | Minimal interactive browser. |
| `doctor` | `[--project P] [--output json\|text] [--auto-repair]` | Diagnostics. Default read-only; `--auto-repair` triggers SQLite `.recover` when corruption detected. |
| `logs` | `[-f] [--lines N]` | Tail `daemon.log`. |
| `team init-ca` | `<output-dir>` | Generate self-signed CA cert + leaf cert for team TCP hosting. Writes PEM files. |
| `team token-create` | `--name N [--admin]` | Generate a per-user bearer token for TCP mode. `--admin` sets `is_admin=true`. |
| `team token-list` | — | List active tokens (names + admin flag, no secret value). |
| `team token-revoke` | `<name>` | Revoke a token by name. |
| `completions` | `{bash,zsh,fish}` | Emit shell completion script to stdout. |
| `version` | — | Print `memlayer X.Y.Z`. |

### 3.4 Exit codes
- `0` success.
- `1` general failure.
- `2` usage error.
- `3` already in target state (e.g. `daemon start` when already running).
- `4` daemon not reachable and auto-spawn failed.
- `5` ambiguous project (recoverable: pass `--project`).

### 3.5 Tab-completion
`memlayer completions {bash,zsh,fish}` emits a completion script via `clap_complete`. Listed in §3.3 as a top-level command (no sub-verb needed).

---

## 4. Daemon

### 4.1 Lifecycle

**Auto-spawn (default).** First CLI invocation:
1. Try to connect to `~/.memlayer/daemon.sock`.
2. On connect refused / file missing → acquire `flock` on `~/.memlayer/daemon.lock`.
3. Re-check the socket (another CLI may have raced and spawned).
4. If still missing, `Command::new(self_path()).arg("daemon").arg("start").spawn()` and detach.
5. Poll for socket readiness with 5s timeout, exponential backoff (10ms, 20ms, …).
6. On readiness, release lock, connect, proceed.
7. On timeout, exit code 4 with diagnostic pointing to `daemon.log`.

**Foreground**: `memlayer daemon start --foreground` runs in the terminal, logs to stderr.

**Service-managed**: `memlayer service install` writes:
- macOS: `~/Library/LaunchAgents/dev.memlayer.daemon.plist` with `RunAtLoad=true`, `KeepAlive.SuccessfulExit=false`.
- Linux: `~/.config/systemd/user/memlayer.service` with `Type=simple`, `Restart=on-failure`, `WantedBy=default.target`.

**Idle shutdown**: none. Daemon stays alive until reboot, OOM, or `daemon stop`.

### 4.2 Socket
- Path: `~/.memlayer/daemon.sock`.
- Mode: `0600`. Owner-only. Set explicitly with `chmod` after bind.
- On startup, daemon unlinks any stale socket, then binds.
- On graceful shutdown (SIGTERM / SIGINT), daemon unlinks the socket and pid file.

### 4.3 Auth & Team Mode

**Local (UDS):** No protocol-level auth. Filesystem permissions on the socket (`0600`) are the entire trust boundary.

**Hosted-team mode (TCP):** When `MEMLAYER_LISTEN=tcp://host:port`, daemon binds TCP instead of UDS. TCP mode:
- Requires mandatory TLS via `MEMLAYER_TLS_CERT` / `MEMLAYER_TLS_KEY`. Refuses to start if either is missing.
- Accepts per-user bearer tokens. Each token is a cryptographically random 32-byte hex string stored in `~/.memlayer/tokens.db` alongside its name and `is_admin` flag. Token passed as gRPC `authorization: Bearer <token>` metadata.
- Token comparison is constant-time.
- Admin-only commands (gated by `is_admin=true`):
  - `team token-create`, `team token-revoke`, `team token-list`
  - `project delete --hard`
  - `daemon stop` (remote shutdown)
  - `daemon restart`
- All other commands require any valid token.

**TLS certificate setup:** `memlayer team init-ca <output-dir>` generates:
- A self-signed CA (`ca.pem`).
- A leaf cert signed by the CA (`server.pem` + `server-key.pem`).
- Clients trust the CA at connection time via `MEMLAYER_TLS_CA`.

**Compression (TCP only):** gzip compression enabled on TCP mode. Not enabled on UDS (already in-process).

### 4.4 Connection & keepalive
- HTTP/2 PING keepalive: every 30s, 20s timeout. Applies to both UDS and TCP.
- Idle connection reap: 10-minute idle timeout on TCP mode. UDS connections held open by default.
- `concurrency_limit_per_connection = 64`, `max_concurrent_streams = 256`, `tcp_nodelay = true` (TCP mode).
- Per-project write channel buffer: 1024 messages.
- Per-project read pool: max 4 concurrent read connections per project (read-only `rusqlite` connections opened with `SQLITE_OPEN_READONLY`).
- LRU project-connection cache: capacity **256**. Eviction triggers connection close. Cache hit ratio surfaced in `daemon status`.

### 4.5 File descriptors
- Estimated peak: 1000 client connections + 256 projects × (1 write conn + 4 read conns + 1 WAL FD) ≈ 2540 FDs.
- Daemon does `setrlimit(RLIMIT_NOFILE, 8192)` at startup (best-effort; warns if it can't).
- `service install` writes `LimitNOFILE=8192` into the unit file.

### 4.6 Signals
| Signal | Behavior |
|---|---|
| `SIGTERM`, `SIGINT` | Drain in-flight requests (5s timeout), flush write thread, close all DBs, unlink socket + pid + lock, exit 0 |
| `SIGHUP` | Reopen `daemon.log` (for log rotation handoff) |
| `SIGUSR1` | Dump diagnostics snapshot to `~/.memlayer/diagnostics-<ts>.json` |

### 4.7 Logging
- Crate: `tracing` + `tracing-subscriber` JSON formatter + `tracing-appender` size-rolling file appender.
- File: `~/.memlayer/daemon.log`.
- Rotation: 10 MB per file, 5 files retained (50 MB cap).
- Log level: `info` default, override via `MEMLAYER_LOG=debug,tonic=warn` (standard `tracing-subscriber` env-filter syntax).
- Per-RPC span includes: rpc name, project, latency, result, client connection id.

### 4.8 Disk-full read-only mode
- Daemon polls free space on `~/.memlayer/` every 30 seconds.
- On free space < 50 MB: daemon enters **read-only mode**.
  - All write RPCs (`SaveObservation`, `UpdateObservation`, `SavePrompt`, `StartSession`, `EndSession`, etc.) return `RESOURCE_EXHAUSTED` with message `"disk full — memlayer is in read-only mode"`.
  - Read RPCs (`SearchObservations`, `ListObservations`, `GetObservation`, etc.) continue to serve.
- On free space recovery (≥ 50 MB): daemon automatically exits read-only mode on next poll cycle.
- Read-only mode is surfaced in `daemon status` and `doctor` output.

---

## 5. Storage

### 5.1 Layout

```
~/.memlayer/
  daemon.sock | daemon.lock | daemon.pid | daemon.log[.1..5]
  config.json                  ← daemon-level config (optional)
  tokens.db                    ← team token store (TCP mode only)
  projects/
    <normalized-project>.db    ← SQLite + FTS5
    <normalized-project>.db-wal
    <normalized-project>.db-shm
    <normalized-project>/
      config.json              ← per-project config (display name, etc.)
  shared/
    sessions.db                ← cross-project session index (deferred to v2)
```

### 5.2 Connection settings (every open)
```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;     -- WAL+NORMAL is durable across crashes, fast
PRAGMA foreign_keys = ON;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;    -- 256 MiB
PRAGMA cache_size = -65536;      -- 64 MiB per connection
PRAGMA busy_timeout = 5000;
```

### 5.3 Schema (per-project DB)

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    directory  TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at   TEXT,
    summary    TEXT
);

CREATE TABLE IF NOT EXISTS observations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_id         TEXT NOT NULL UNIQUE,
    session_id      TEXT NOT NULL,
    type            TEXT NOT NULL,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL,
    tool_name       TEXT,
    scope           TEXT NOT NULL DEFAULT 'project',  -- 'project' | 'personal' | 'team'
    created_by      TEXT,                             -- user@host for scope-aware queries
    topic_key       TEXT,
    normalized_hash TEXT,
    revision_count  INTEGER NOT NULL DEFAULT 1,
    duplicate_count INTEGER NOT NULL DEFAULT 1,
    last_seen_at    TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at      TEXT,
    review_after    TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_obs_session  ON observations(session_id);
CREATE INDEX IF NOT EXISTS idx_obs_type     ON observations(type);
CREATE INDEX IF NOT EXISTS idx_obs_created  ON observations(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_obs_scope    ON observations(scope);
CREATE INDEX IF NOT EXISTS idx_obs_created_by ON observations(created_by) WHERE created_by IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_obs_topic    ON observations(topic_key, scope, updated_at DESC) WHERE topic_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_obs_dedupe   ON observations(normalized_hash, created_at DESC) WHERE normalized_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_obs_active   ON observations(deleted_at) WHERE deleted_at IS NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    title, content, tool_name, type, topic_key,
    content='observations', content_rowid='id'
);

CREATE TRIGGER obs_fts_ai AFTER INSERT ON observations BEGIN
  INSERT INTO observations_fts(rowid, title, content, tool_name, type, topic_key)
  VALUES (new.id, new.title, new.content, new.tool_name, new.type, new.topic_key);
END;
CREATE TRIGGER obs_fts_ad AFTER DELETE ON observations BEGIN
  INSERT INTO observations_fts(observations_fts, rowid, title, content, tool_name, type, topic_key)
  VALUES ('delete', old.id, old.title, old.content, old.tool_name, old.type, old.topic_key);
END;
CREATE TRIGGER obs_fts_au AFTER UPDATE ON observations BEGIN
  INSERT INTO observations_fts(observations_fts, rowid, title, content, tool_name, type, topic_key)
  VALUES ('delete', old.id, old.title, old.content, old.tool_name, old.type, old.topic_key);
  INSERT INTO observations_fts(rowid, title, content, tool_name, type, topic_key)
  VALUES (new.id, new.title, new.content, new.tool_name, new.type, new.topic_key);
END;

CREATE TABLE IF NOT EXISTS user_prompts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_id    TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);
CREATE INDEX IF NOT EXISTS idx_prompts_session ON user_prompts(session_id);
CREATE INDEX IF NOT EXISTS idx_prompts_created ON user_prompts(created_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
    content,
    content='user_prompts', content_rowid='id'
);

CREATE TRIGGER prompt_fts_ai AFTER INSERT ON user_prompts BEGIN
  INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER prompt_fts_ad AFTER DELETE ON user_prompts BEGIN
  INSERT INTO prompts_fts(prompts_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;
CREATE TRIGGER prompt_fts_au AFTER UPDATE ON user_prompts BEGIN
  INSERT INTO prompts_fts(prompts_fts, rowid, content) VALUES ('delete', old.id, old.content);
  INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TABLE IF NOT EXISTS sync_chunks (
    chunk_id    TEXT PRIMARY KEY,
    imported_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', '1');
```

**Schema notes**:
- `project` column dropped from observations/sessions/prompts (project is implicit in the file path).
- `embedding`, `embedding_model`, `embedding_created_at` columns are not included (no vector search in v1).
- `scope` column: `'project' | 'personal' | 'team'`. `created_by` enables per-user filtering in team contexts.
- No `memory_relations`, no conflict-relations state machine, no cloud sync tables.
- `prompt_tombstones` table omitted (prompts are hard-delete only; no need for tombstone tracking).

### 5.4 Migrations (refinery)
- Schema versions tracked via `schema_meta(key='version')`.
- All migrations are embedded SQL files in `migrations/` and loaded at compile time with `refinery::embed_migrations!`.
- On daemon startup (or first DB open per project), `refinery::migrate_database` runs before any other operation.
- Migrations are append-only. Never edit a migration that has been committed to `main`.

### 5.5 Cross-project queries
- Cross-project search is not the hot path. When invoked (`obs search --all-projects`), the daemon `ATTACH`es each project DB and runs a `UNION ALL` over FTS5. Capped at 32 simultaneous attaches.
- `shared/sessions.db` (deferred to v2) would hold a `(session_id, project)` index for cross-project session lookups. Not built in v1 — sessions are project-scoped.

### 5.6 Per-project config
`~/.memlayer/projects/<normalized>/config.json`:
```json
{
  "project_display_name": "My Actual Repo Name",
  "created_at": "<RFC3339>"
}
```
- `project_display_name` is the original name as provided before normalization.
- On `project create` (implicit via first save), daemon checks if `project_display_name` would normalize to an already-claimed `<normalized>` key. If different originals collide on the same normalized key, daemon returns `ALREADY_EXISTS` with a message describing the conflict — no silent data merge.

### 5.7 Write thread, batched commits
- One write thread per project (lazily started on first write).
- Receives messages on a `tokio::sync::mpsc` channel, buffer 1024.
- Drains up to **N=32 messages** or **5ms** (whichever first), wraps them in `BEGIN IMMEDIATE; …; COMMIT;`, flushes.
- Single-project burst write target (1000 saves/sec): batch size ≈ 5–10, commit cycle ≈ 5ms, well under the 100ms p95 ceiling.
- On any single-write error, the entire batch rolls back and each caller gets the original error (gRPC `Internal` with detail).

### 5.8 Dedupe
- Window: 15 minutes (`MEMLAYER_DEDUPE_WINDOW`, default `15m`, `0` to disable).
- On `obs save`:
  1. Compute `normalized_hash = sha256(lowercase(collapse_whitespace(content)))`.
  2. If `topic_key` set: upsert by `(scope, topic_key)`. Matches → update content + `revision_count += 1`. Bump `updated_at`.
  3. Else lookup `(normalized_hash, scope, type, title, created_at >= now()-window)`. Match → bump `duplicate_count`, `last_seen_at`, `updated_at`. Return the existing row.
  4. Else insert new.
- `sync_id` is the idempotency key for external callers. If a `SaveObservation` request arrives with a `sync_id` that already exists in the DB, the daemon returns the existing row without modification and without error. Callers retrying on timeout get the same response as the original write.

### 5.9 Soft-delete
- `obs delete <id>` sets `deleted_at = datetime('now')`.
- `obs delete <id> --hard` removes the row.
- All search/list/context queries filter `deleted_at IS NULL`. The partial `idx_obs_active` makes the filter free.

### 5.10 Review schedule
On insert, derive `review_after`:
- `decision` → `now()+6 months`
- `policy` → `now()+12 months`
- `preference` → `now()+3 months`
- else → NULL

Surfaced via `obs list --due-for-review`.

---

## 6. gRPC Service

### 6.1 Single flat service
```protobuf
syntax = "proto3";
package memlayer.v1;

service Memlayer {
  // Observations
  rpc SaveObservation        (SaveObservationRequest)        returns (SaveObservationResponse);
  rpc UpdateObservation      (UpdateObservationRequest)      returns (Observation);
  rpc DeleteObservation      (DeleteObservationRequest)      returns (DeleteResponse);
  rpc GetObservation         (IdRequest)                     returns (Observation);
  rpc SearchObservations     (SearchRequest)                 returns (SearchResponse);
  rpc ListObservations       (ListObservationsRequest)       returns (ListObservationsResponse);
  rpc RecentObservations     (ListObservationsRequest)       returns (ListObservationsResponse);
  rpc Context                (ContextRequest)                returns (ContextResponse);
  rpc Timeline               (TimelineRequest)               returns (TimelineResponse);
  rpc SuggestTopicKey        (SuggestTopicKeyRequest)        returns (SuggestTopicKeyResponse);
  rpc CapturePassive         (CapturePassiveRequest)         returns (CapturePassiveResponse);

  // Sessions
  rpc StartSession           (StartSessionRequest)           returns (StartSessionResponse);
  rpc EndSession             (EndSessionRequest)             returns (Session);
  rpc SaveSessionSummary     (SessionSummaryRequest)         returns (Session);
  rpc ListSessions           (ListSessionsRequest)           returns (ListSessionsResponse);
  rpc GetSession             (IdRequest)                     returns (Session);
  rpc DeleteSession          (IdRequest)                     returns (DeleteResponse);

  // Prompts
  rpc SavePrompt             (SavePromptRequest)             returns (Prompt);
  rpc SearchPrompts          (SearchRequest)                 returns (SearchResponse);
  rpc RecentPrompts          (ListPromptsRequest)            returns (ListPromptsResponse);
  rpc DeletePrompt           (IdRequest)                     returns (DeleteResponse);

  // Projects
  rpc ListProjects           (Empty)                         returns (ListProjectsResponse);
  rpc CurrentProject         (CurrentProjectRequest)         returns (CurrentProjectResponse);
  rpc MergeProjects          (MergeProjectsRequest)          returns (MergeProjectsResponse);
  rpc DeleteProject          (DeleteProjectRequest)          returns (DeleteResponse);

  // Sync
  rpc SyncExport             (SyncExportRequest)             returns (SyncExportResponse);
  rpc SyncImport             (Empty)                         returns (SyncImportResponse);
  rpc SyncStatus             (SyncStatusRequest)             returns (SyncStatusResponse);

  // Team
  rpc CreateToken            (CreateTokenRequest)            returns (CreateTokenResponse);
  rpc ListTokens             (Empty)                         returns (ListTokensResponse);
  rpc RevokeToken            (RevokeTokenRequest)            returns (Empty);

  // Diagnostics
  rpc Stats                  (StatsRequest)                  returns (StatsResponse);
  rpc Doctor                 (DoctorRequest)                 returns (DoctorResponse);
  rpc DaemonStatus           (Empty)                         returns (DaemonStatusResponse);
  rpc Shutdown               (Empty)                         returns (Empty);
}
```

**`StartSessionResponse`** includes both the session object and the inline context (equivalent to a `ContextResponse`) so agents do not need a separate `Context` RPC call at session start.

Field-level message definitions are derived directly from §5.3 column names + §3.3 flags. The CLI marshals into these messages 1:1.

### 6.2 Errors
Standard gRPC status codes + structured JSON detail:

```json
{"code": "NOT_FOUND", "message": "observation 42 does not exist", "details": {"id": 42}}
```

- `INVALID_ARGUMENT` — malformed input.
- `NOT_FOUND` — id doesn't exist.
- `ALREADY_EXISTS` — sync_id collision on import, or project display-name collision.
- `FAILED_PRECONDITION` — e.g. `DeleteSession` with referencing observations.
- `UNAVAILABLE` — daemon overloaded (back-pressure) or in read-only mode.
- `RESOURCE_EXHAUSTED` — disk full (triggers read-only mode).
- `PERMISSION_DENIED` — TCP mode, non-admin token attempting admin operation.
- `UNAUTHENTICATED` — TCP mode, missing or invalid token.
- `INTERNAL` — anything unexpected (logged with full context).

Error detail is a JSON object in the gRPC `status.details` field, encoded as `google.protobuf.Any` wrapping `google.rpc.ErrorInfo`. For now: proto projection from a hand-curated struct. Breaking changes to the error shape will move to `memlayer.v2` (see §6.3).

### 6.3 Versioning
- Package is `memlayer.v1`.
- Backwards-incompatible changes go to `memlayer.v2`. Daemon hosts both during transition.

### 6.4 Pagination
- List RPCs use cursor-based pagination.
- Cursor is an opaque base64-encoded JSON token: `{"version": 1, "last_id": 123, "last_created_at": "<RFC3339>"}`.
- Passing `cursor` to a list RPC returns the next page. No cursor = first page.
- Response includes `next_cursor: string | null` (null when no more pages).

---

## 7. Sync (Git-Based)

### 7.1 Layout
```
<repo>/.memlayer/
  manifest.json
  chunks/
    <chunk_id>.jsonl.zst
  config.json    (optional)
```

`config.json` shape: `{"project_name": "..."}`.

### 7.2 Manifest
```json
{
  "version": 1,
  "chunks": [
    {
      "id": "a3f8c1d2",
      "created_by": "<user@host>",
      "created_at": "<RFC3339>",
      "sessions": 2,
      "observations": 15,
      "prompts": 3,
      "size_bytes": 4321
    }
  ]
}
```

Append-only; mergeable via standard git 3-way merge.

### 7.3 Chunk
- Filename: `<chunk_id>.jsonl.zst`.
- `chunk_id`: first 16 hex chars of `sha256(uncompressed_jsonl_body)`.
- Compression: zstd level 19 (high compression, still fast).
- Body: one JSON object per line, line types `session | observation | prompt`.

```jsonl
{"type":"session","data":{"sync_id":"…","id":"…","directory":"…","started_at":"…","ended_at":null,"summary":"…","deleted":false,"deleted_at":null,"hard_delete":false}}
{"type":"observation","data":{"sync_id":"…","session_id":"…","type":"…","title":"…","content":"…","tool_name":null,"scope":"project","created_by":"user@host","topic_key":"…","revision_count":1,"duplicate_count":1,"last_seen_at":null,"created_at":"…","updated_at":"…","deleted":false,"deleted_at":null,"hard_delete":false}}
{"type":"prompt","data":{"sync_id":"…","session_id":"…","content":"…","created_at":"…"}}
```

`project` is **not** a payload field — it's implicit in which DB the chunk imports into (the project name comes from the manifest path or `config.json`).

### 7.4 Export
1. Resolve project (`--all` or detected).
2. Select rows where `sync_id NOT IN (chunked rows from sync_chunks)`. Track via a side table or by querying manifest content if needed.
3. Build JSONL, zstd-compress, hash → `<chunk_id>`.
4. Write `<repo>/.memlayer/chunks/<chunk_id>.jsonl.zst`.
5. Append manifest entry.
6. Insert `(chunk_id)` into `sync_chunks` (per-project DB).

### 7.5 Import
1. Read manifest.
2. For each chunk not in `sync_chunks`:
   - Decompress, parse JSONL.
   - For each row: upsert by `sync_id`. Conflict resolution: take the row with the later `updated_at`.
   - Insert chunk_id into `sync_chunks` after successful apply.
3. Return `{chunks_imported, sessions_imported, observations_imported, prompts_imported}`.

### 7.6 Conflict avoidance
- Chunks immutable.
- Manifest append-only.
- Imports idempotent on `chunk_id`.
- No git-merge conflict possible on the sync side.

---

## 8. Markdown Export & Import (Round-trippable)

### 8.1 Layout
```
<dir>/
  manifest.json
  observations/<sync_id>.md
  prompts/<sync_id>.md
  sessions/<sync_id>.md
```

### 8.2 manifest.json
```json
{
  "version": 1,
  "exported_at": "<RFC3339>",
  "project": "memlayer",
  "counts": { "observations": 100, "prompts": 20, "sessions": 5 }
}
```

### 8.3 File format
```markdown
---
sync_id: 7f2c1d4a-...
session_id: a1b2c3...
type: bugfix
title: Fixed FTS5 syntax error
scope: project
created_by: user@host
topic_key: bug/fts5-syntax
created_at: 2026-01-15T10:00:00Z
updated_at: 2026-01-15T10:30:00Z
revision_count: 2
duplicate_count: 1
deleted: false
---
[Observation content as markdown]
```

### 8.4 Round-trip rules
- Filename = `<sync_id>.md` (stable across re-export).
- Import upserts on `sync_id`.
- Conflict resolution: frontmatter `updated_at` newer → overwrite local.
- Integer `id` is intentionally omitted (DB-local).
- `--since DATE` exports only rows with `updated_at >= DATE`.
- Editing a `.md` file by hand and re-importing is a supported workflow; bumping `updated_at` is the user's responsibility.

---

## 9. Project Detection

Three cases only. No "scan for child repos." No "dir basename fallback."

### 9.1 Algorithm
Given `cwd`:
1. **Case 0 — config file.** Walk up from `cwd` to enclosing git root. If `.memlayer/config.json` exists, return `project_name`. Source = `config`.
2. **Case 1 — git remote.** If `cwd` is inside a git repo with `origin` remote, parse the remote URL (last path segment, strip `.git`). Source = `git_remote`. Supported URL forms: `git@host:org/repo.git`, `https://host/org/repo[.git]`, `ssh://host/org/repo.git`.
3. **Case 2 — git root.** Git repo without `origin`: use repo root basename. Source = `git_root`.
4. **Else**: error. Exit code 5. Message:
   ```
   memlayer: cannot determine project — current directory is not a git repo
   and contains no .memlayer/config.json. Either run from inside a git repo,
   create .memlayer/config.json with {"project_name":"<name>"}, or pass
   --project <name>.
   ```

### 9.2 Normalization
- Trim, lowercase, replace spaces/underscores with `-`, drop characters outside `[A-Za-z0-9._-]`.
- Reject normalized names that begin with `.` or contain `..` (path safety).
- File on disk: `~/.memlayer/projects/<normalized>.db`.
- On first project creation, `project_display_name` (the pre-normalization original) is stored in `~/.memlayer/projects/<normalized>/config.json`. If a subsequent project would normalize to the same key from a different original display name, daemon returns `ALREADY_EXISTS` and requires disambiguation via `--project`.

### 9.3 Similar matching
`project consolidate` finds candidates by Levenshtein distance ≥ 80% similarity. No "shared directory" heuristic (sessions are now per-project; cross-project directory join is not meaningful).

---

## 10. Setup (Agent Prompt-Level Integrations)

### 10.1 Behavior
- `memlayer setup` (no args): scan for installed agents (look for marker files), present a multi-select picker, ask "global or project?" per selection, write integration files.
- `memlayer setup <agent>`: skip detection, prompt scope only.
- `memlayer setup <agent> --scope global|project`: fully non-interactive.
- `memlayer setup --print <agent>`: emit the prompt content to stdout for paste-in.

### 10.2 Detection markers
| Agent | Marker (presence indicates installed) |
|---|---|
| claude-code | `~/.claude/` |
| cursor | `~/.cursor/` or `~/Library/Application Support/Cursor/` |
| opencode | `~/.config/opencode/` |
| codex | `~/.codex/` |
| gemini-cli | `~/.gemini/` |

### 10.3 Output paths
| Agent | Global | Project |
|---|---|---|
| claude-code | `~/.claude/skills/memlayer/SKILL.md` | `<repo>/.claude/skills/memlayer/SKILL.md` |
| cursor | `~/.cursor/rules/memlayer.md` | `<repo>/.cursor/rules/memlayer.md` |
| opencode | `~/.config/opencode/system-prompt.d/memlayer.md` | `<repo>/.opencode/system-prompt.d/memlayer.md` |
| codex | `~/.codex/instructions.d/memlayer.md` | `<repo>/.codex/instructions.d/memlayer.md` |
| gemini-cli | `~/.gemini/instructions.d/memlayer.md` | `<repo>/.gemini/instructions.d/memlayer.md` |

### 10.4 Skill content — Detailed template (memory protocol)
The skill template enforces a strict agent protocol. It contains:

**Session lifecycle:**
- At the start of every session, run `memlayer session start <session-id> --directory <cwd>`. The response includes an inline context summary (recent observations, decisions, active topic keys). Prepend this context to the agent's working context before beginning work.
- At the end of every session, run `memlayer session end <session-id> --summary "<one-sentence summary>"`.

**When to save an observation:**
- After fixing a bug: `--type bugfix`
- After making an architectural decision: `--type decision`
- After discovering a pattern: `--type pattern`
- After changing a preference or config convention: `--type preference`
- When recording a policy or standard: `--type policy`

**Save format:**
```
memlayer obs save \
  --title "<short imperative title>" \
  --type <type> \
  --scope <project|personal|team> \
  --content "**What**: <what was done>\n**Why**: <motivation>\n**Where**: <file or component>\n**Learned**: <the insight>"
```

**Topic-key conventions:**
- Format: `<family>/<slug>` (two levels max, lowercase kebab).
- Families: `architecture/`, `bug/`, `decision/`, `pattern/`, `config/`, `discovery/`, `learning/`.
- Use topic keys for evolving facts (e.g., `architecture/auth-strategy`). Each save with the same key updates the record in-place.

**Search-first protocol:**
- Before starting significant work, run `memlayer obs search "<task keywords>"` to surface relevant prior observations.
- Agents must not duplicate observations that already exist — search first, save if new.

**Output format note:**
- When piped (i.e., called from an agent script), `memlayer` outputs JSON automatically. No `--output json` flag needed in most cases.

### 10.5 Idempotency
- Re-running `setup` checks for existing file. If identical → no-op. If different → diff and prompt (unless `--force`).
- All writes are atomic (write to `<path>.tmp`, then rename).

---

## 11. TUI

### 11.1 Scope
Single-screen interactive browser with inline edit support. ~500 LOC ceiling.

### 11.2 Layout
```
┌────────────────────────────────────────────────┐
│ Search: [_______________]              memlayer│
├────────────────────────────────────────────────┤
│ > [42] bugfix : Fixed FTS5 syntax error        │
│   [41] decision : Use Tantivy if SQLite slow   │
│   [40] pattern : Topic-key naming              │
│   …                                             │
├────────────────────────────────────────────────┤
│ # Fixed FTS5 syntax error                      │
│ **What**: ...                                   │
│ **Why**: ...                                    │
│ **Where**: ...                                  │
│ **Learned**: ...                                │
└────────────────────────────────────────────────┘
 j/k navigate • enter detail • / search • e edit • d delete • c copy • q quit
```

### 11.3 Keys
| Key | Action |
|---|---|
| `/` | Focus search input |
| `j` / `↓` | Cursor down |
| `k` / `↑` | Cursor up |
| `Enter` | Toggle detail pane (full content) |
| `e` | Open current observation in `$EDITOR`. On save-and-quit, update via `obs update`. |
| `d` | Soft-delete current observation (with confirmation prompt). |
| `c` | Copy current observation content (OSC 52 escape) |
| `r` | Refresh (re-run search) |
| `q` / `Esc` | Quit |

### 11.4 Theme
Dark, single palette inspired by Catppuccin Mocha (no theme switching in v1). Color via `ratatui::style::Color`.

### 11.5 Implementation note
Built on `ratatui` + `crossterm`. Uses async `tokio` for incremental search (debounce 150ms). Connects to the daemon over the same gRPC client used by the CLI. Editor integration suspends the TUI, spawns `$EDITOR` in a child process, and resumes TUI on exit.

---

## 12. Doctor & Logs

### 12.1 `memlayer doctor`
Default: read-only diagnostics. `--auto-repair` enables repair.

**Read-only checks** (always run):
- `PRAGMA integrity_check` per project DB.
- FTS5 row count vs. observations row count (drift detection).
- Recent error tail from `daemon.log` (last 10 lines at ERROR level).
- Disk-free space on `~/.memlayer/` mount.
- Daemon read-only mode status.

**Auto-repair** (`--auto-repair`):
- Creates a timestamped backup of the target DB (`<name>.db.bak-<ts>`) before any repair.
- If `integrity_check` fails: runs SQLite `.recover` to reconstruct the DB.
- If FTS5 drift > 0: runs `INSERT INTO observations_fts(observations_fts) VALUES ('rebuild')`.

Output (text or JSON) per project:

```json
{
  "daemon": {
    "uptime_seconds": 12345,
    "active_connections": 12,
    "open_project_dbs": 5,
    "lru_evictions_per_minute": 0,
    "read_only_mode": false,
    "disk_free_bytes": 5368709120
  },
  "projects": [
    {
      "name": "memlayer",
      "display_name": "memlayer",
      "db_path": "~/.memlayer/projects/memlayer.db",
      "size_bytes": 5242880,
      "integrity_check": "ok",
      "counts": { "observations": 142, "sessions": 12, "prompts": 24, "soft_deleted": 3 },
      "fts_coverage": { "observations": 142, "indexed": 142, "drift": 0 },
      "last_error": null
    }
  ]
}
```

### 12.2 `memlayer logs`
- `memlayer logs` — print last 100 lines.
- `memlayer logs -f` — tail follow.
- `memlayer logs --lines 1000` — print last N.
- Output format follows `--output` (JSON lines pass through; text mode pretty-prints).

---

## 13. Configuration Reference

### 13.1 Environment variables

| Var | Purpose | Default |
|---|---|---|
| `MEMLAYER_DATA_DIR` | Override data dir | `~/.memlayer` |
| `MEMLAYER_PROJECT` | Override project detection | (none) |
| `MEMLAYER_LOG` | `tracing-subscriber` env-filter | `info` |
| `MEMLAYER_DEDUPE_WINDOW` | Dedupe window | `15m` |
| `MEMLAYER_LISTEN` | Daemon listen address (`unix:///path` or `tcp://host:port`) | `unix://~/.memlayer/daemon.sock` |
| `MEMLAYER_TOKEN` | Bearer token for TCP mode clients | (none; required if TCP) |
| `MEMLAYER_TLS_CERT` / `_KEY` | TLS for TCP mode server | (none; required if TCP) |
| `MEMLAYER_TLS_CA` | CA cert for TCP mode clients | (none; required if TCP) |
| `NO_COLOR` / `CLICOLOR_FORCE` | Color output control | (standard semantics) |

### 13.2 Files

| Path | Purpose |
|---|---|
| `~/.memlayer/daemon.sock` | UDS for CLI ↔ daemon |
| `~/.memlayer/daemon.lock` | flock for spawn race |
| `~/.memlayer/daemon.pid` | Daemon PID |
| `~/.memlayer/daemon.log[.1..5]` | Rotating JSON logs |
| `~/.memlayer/config.json` | Optional daemon config |
| `~/.memlayer/tokens.db` | Team token store (TCP mode only) |
| `~/.memlayer/projects/<name>.db` | Per-project SQLite + FTS5 |
| `~/.memlayer/projects/<name>/config.json` | Per-project display name + metadata |
| `<repo>/.memlayer/manifest.json` | Sync manifest |
| `<repo>/.memlayer/chunks/*.jsonl.zst` | Sync chunks |
| `<repo>/.memlayer/config.json` | Optional project name lock |

### 13.3 Defaults

```
DataDir              = ~/.memlayer
DedupeWindow         = 15 minutes
MaxObservationLength = 50_000 chars
MaxSearchLimit       = 50
DefaultSearchLimit   = 10
LRUProjectCacheSize  = 256
WriteBatchSize       = 32 messages or 5ms
RuntimeWorkers       = num_cpus
NoFileSoftLimit      = 8192
SocketMode           = 0o600
ZstdLevel            = 19
ChunkIdLength        = 16 hex chars
DiskFreeThreshold    = 50 MB
DiskPollInterval     = 30 seconds
KeepaliveInterval    = 30 seconds
KeepaliveTimeout     = 20 seconds
IdleConnTimeout      = 10 minutes (TCP mode)
```

---

## 14. Build & Distribution

### 14.1 Targets
- `linux-amd64`, `linux-arm64`
- `darwin-amd64`, `darwin-arm64`

### 14.2 Toolchain
- Rust stable (MSRV pinned in `Cargo.toml`).
- `cross` for cross-compilation on Linux runners.
- Native macOS runners for `darwin-*` (codesigning).

### 14.3 Codesigning
- macOS arm64 binaries ad-hoc resigned post-strip via `codesign --force --sign -` to prevent AMFI SIGKILL on macOS ≥ 14.
- No notarization in v1; users on Gatekeeper add `xattr -d com.apple.quarantine` if installed via curl.

### 14.4 Distribution channels
- GitHub Releases: `tar.gz` per target containing the binary + `LICENSE-MIT` + `LICENSE-APACHE` + a `README.md`.
- Homebrew tap (formula points at GitHub Releases).
- One-line installer: `curl -sSf https://memlayer.dev/install.sh | sh` (writes to `/usr/local/bin/memlayer` or `~/.local/bin/memlayer`).
- `cargo install memlayer` deferred to post-v1. crates.io publishing is not part of the v1 release.

### 14.5 Binary size
- Target: < 25 MB stripped per binary.
- Levers if exceeded: `strip = true` and `lto = "fat"` in `Cargo.toml [profile.release]`; vendor only the SQLite extensions actually used (`bundled-full` ≈ 5 MB on its own).

### 14.6 Versioning
- SemVer.
- `memlayer version` prints `memlayer X.Y.Z (commit <sha>, built <date>)`.
- Set via `build.rs` with `vergen` crate.

---

## 15. Test Strategy & Acceptance Bar

Designed for the **1000-concurrent-agent** target.

### 15.1 Test layers

**Unit tests** — every module. ≥80% line coverage on `storage`, `sync`, `daemon::write_thread`. `cargo test` runs in <30s.

**Integration tests** — spawn the binary against a temp data dir, run end-to-end CLI scenarios. Cover every subcommand in §3.3 with positive + negative paths. Use `assert_cmd` + `predicates`.

**Round-trip tests** — for every export format (json, markdown, sync chunks): export → wipe DB → import → diff must be empty.

**Chaos tests** — `SIGKILL` daemon mid-burst, restart, verify recovery. Fill disk, verify graceful failure + read-only mode.

**Concurrency / load tests** — separate binary `memlayer-loadtest` simulates N agents.

**Soak tests** — tiered: 1h smoke, 24h nightly, 7d weekly.

### 15.2 Performance budgets (CI-gated)

| Scenario | Target |
|---|---|
| `obs save` end-to-end, daemon cold | p95 < 250ms (one-time spawn cost) |
| `obs save` end-to-end, daemon warm | p95 < 10ms |
| `obs save` server-side handler | p95 < 2ms, p99 < 5ms |
| `obs search` over 100K rows | p95 < 30ms |
| `obs search` over 1M rows | p95 < 100ms, p99 < 250ms |
| Daemon cold-start (auto-spawn → socket accepts) | < 200ms |
| Daemon idle RSS | < 50 MB |
| Daemon RSS with 256 hot projects | < 500 MB |
| Binary size stripped | < 25 MB |

### 15.3 Concurrency budgets — 1000 agents

| Scenario | Target |
|---|---|
| 1000 sustained concurrent gRPC connections | 0 connection errors over 1h |
| 1000 agents × 1 save/sec across 1000 distinct projects | p95 < 20ms, p99 < 50ms, 0 errors over 1h |
| 1000 agents × 1 save/sec on **one shared project** | p95 < 100ms, p99 < 250ms, 0 errors |
| 1000 agents × 1 search/sec | p95 < 150ms, 0 errors |
| 100 concurrent `sync export` invocations | all succeed, no chunk corruption |
| LRU eviction churn under 1000 hot projects | < 10 evictions/min after warmup |
| FD leak rate | 0 FDs leaked per 1000 RPCs |

### 15.4 Tiered soak schedule

| Tier | Duration | Trigger | Load |
|---|---|---|---|
| Smoke | 1h | Every PR | 100 agents, mixed save/search at 5 RPC/sec |
| Nightly | 24h | Nightly CI | 1000 agents, mixed save/search at 5 RPC/sec |
| Weekly | 7d | Weekly CI | 1000 agents, sustained |

Acceptance criteria per tier:
- RSS growth ≤ 20% from minute-30 baseline.
- FD count stable (`lsof -p $DAEMON_PID | wc -l`).
- 0 unhandled errors at ERROR level in `daemon.log`.
- p95 latency at end of run within 2× of first-30-minutes baseline.

### 15.5 Tiered scaling guide
When aggregate write rate approaches the 5K saves/sec ceiling:
1. **Increase batch size** (`WriteBatchSize` up to 128) — free headroom.
2. **Shard hot projects** — split by `created_by` or `scope` into child DB files.
3. **Increase write thread concurrency** per project (current: 1 per project; this is safe to raise to 2 with WAL).
4. **Migrate hot project to RocksDB** — escape hatch for v2 if SQLite ceiling is definitively hit.

### 15.6 Chaos
- `SIGKILL` daemon during 100-save burst → on restart, no torn records, FTS5 in sync with row store.
- Disk full → read-only mode, `RESOURCE_EXHAUSTED` returned to client; daemon stays up; auto-exits read-only on recovery.
- 5000 connections in 1 second → `UNAVAILABLE` returned (back-pressure); daemon recovers when load drops.
- Corrupt chunk file → import skips with explicit error; doesn't poison DB.
- Stale socket file (daemon crashed) → next CLI auto-spawn unlinks and rebinds.

### 15.7 Tooling deliverables
- `memlayer-loadtest` separate binary: configurable agent count, RPC rate, project distribution, duration. Outputs latency histogram + error counts in JSON.
- GitHub Actions: perf + concurrency (1h smoke) on every PR; merge blocked on >10% regression vs `main`.
- Nightly soak workflow: 24h run, posts results to a tracking issue.
- Weekly soak workflow: 7d run, results archived.
- Flamegraph workflow: weekly `cargo flamegraph` on the standard load profile, archived for review.

### 15.8 Acceptance criteria for v1.0.0
All of the following hold:
1. Every CLI command in §3.3 has both a passing positive integration test and at least one negative test.
2. Every gRPC RPC in §6.1 has a passing happy-path test.
3. Every performance budget in §15.2 met on the CI runner (linux-amd64).
4. Every concurrency budget in §15.3 met by `memlayer-loadtest` on a 8-core 16GB CI runner.
5. 1h smoke soak completes with no leaks; 24h nightly passes before v1.0.0 tag.
6. Single binary < 25MB on all four targets.
7. `setup` writes byte-identical files for re-run on the same machine; preserves user content outside the memlayer block.
8. Round-trip tests pass for json, markdown, and sync chunks.
9. `doctor --auto-repair` recovers a synthetically corrupted DB in the chaos test suite.

---

## 16. Risks & Open Questions

### 16.1 Open questions (decisions deferred)
1. **Single-shared-project p95 = 100ms acceptable?** Hitting <20ms on a single shared project requires moving off SQLite for that hot path. v1 ships with the 100ms target; revisit if real workloads hit the ceiling.
2. **Aggregate save ceiling: 5K/sec.** No firm agent-population data yet to validate. Re-measure on first hosted-team deployment.
3. **`shared/sessions.db` cross-project session index.** Deferred to v2 unless a real workload demands it.
4. **crates.io publishing.** Deferred to post-v1. GitHub Releases + Homebrew tap covers initial distribution.
5. **7d soak as v1 gate?** Weekly 7d soak runs nightly before release; it is an ongoing health signal, not a hard ship gate for v1.0.0.

### 16.2 Risks
1. **`rusqlite::bundled-full` adds ~5 MB to binary.** Acceptable within 25 MB budget. If exceeded, downgrade to `bundled` (without all extensions).
2. **`zstd` level 19 is CPU-heavy on export.** Mitigation: export is rare, runs in a tokio blocking task. Drop to level 9 if export latency becomes user-visible.
3. **OSC 52 clipboard may not work in all terminals.** Fallback: print "copy this:" + content if OSC 52 fails. Matches `ratatui` ecosystem behavior.
4. **`getrlimit`/`setrlimit` on macOS caps soft limit at 256 by default.** `service install` writes the hard limit; auto-spawned daemon best-effort raises to 8192 and warns if it can't.
5. **Cross-project search via `ATTACH` has a 10-database SQLite default cap** (`SQLITE_MAX_ATTACHED`). `bundled-full` raises this to 125. Capping fan-out at 32 is well within.
6. **Tonic stream limits.** `max_concurrent_streams = 256` per connection; with 1000 connections that's 256K total streams. Tokio handles this on modern kernels with `ulimit -n 8192`.
7. **Project display-name collision.** Two repos that normalize to the same slug will get `ALREADY_EXISTS`. Users must either rename one or set explicit `project_name` in `.memlayer/config.json`.
8. **rcgen TLS cert rotation.** Self-signed certs have a fixed expiry. `team init-ca` will need a re-cert workflow in a future version; v1 certs are valid 10 years by default.

---

## 17. Out-of-PRD (deliberately not specified)
- Specific log message strings (left to implementation).
- Skill template exact wording (subject to UX iteration during build; §10.4 specifies structure and required sections, not verbatim copy).
- gRPC message field-level semantics not derivable from §5.3 / §3.3 (treated as obvious 1:1 mappings; codified in `.proto` during implementation).
- Internal module structure of the Rust crate.
- Specific Levenshtein algorithm crate (any reasonable choice; locked at implementation time).
- Token storage schema in `tokens.db` (implementation detail; single-table SQLite).

