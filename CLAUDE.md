# memlayer — contributor notes for AI coding agents

## Architecture in one paragraph

The `memlayer` binary is a thin CLI (`crates/memlayer-cli`) that auto-spawns a
per-user gRPC daemon (`crates/memlayer-daemon`) on first call. The daemon owns
one SQLite+FTS5 per-project DB (`crates/memlayer-storage`), a cross-project
BM25 mirror (`global.sqlite`), and per-project write threads. Two async worker
pools run off the save hot path: an embed worker (BGE-small, candle-rs) and an
extract worker (Claude shell-out). Retrieval uses BM25 only or BM25 + dense ANN
(sqlite-vec) fused via RRF (`crates/memlayer-retrieval`). The wire protocol is
proto3 gRPC (`proto/memlayer.proto`).

## Crate dependency order (low → high)

```
memlayer-core              (config, paths, error)
memlayer-proto             (tonic-generated stubs)
memlayer-storage           (rusqlite, facts, conflict_judge trait)
memlayer-embed             (BgeSmallEmbedder, quantize)
memlayer-extract           (ClaudeClient, ClaudeCliExtractor, extractor)
memlayer-retrieval         (rrf_fuse, rrf_fuse_keyed, ClaudeReranker, HybridMode)
memlayer-daemon            (service, server, embed_worker, extract_worker, conflict_judge impl)
memlayer-client            (channel builders: UDS + TCP)
memlayer-cli               (memlayer binary)
memlayer-sync              (export/import)
memlayer-eval              (benchmark harness — not imported by daemon)
memlayer-tests             (integration tests)
```

## Key conventions

- **Migrations** live in `migrations/V{N}__name.sql`, embedded at compile time
  via `refinery::embed_migrations!`. Current head: V6 (embeddings_quantize).
  Always use `IF NOT EXISTS` / `ALTER TABLE ... ADD COLUMN` for idempotency.
- **Write thread** per project: `storage::write::spawn_write_thread`. All DB
  mutations go through `WriteRequest` variants on a `crossbeam` bounded channel.
  Use `WriteRequest::Custom` for one-off operations.
- **Config** (`MemlayerConfig`) uses 3-level TOML merge: env vars >
  `~/.memlayer/projects/<name>.config.toml` > `~/.memlayer/config.toml` >
  code defaults. Re-resolve per task — never cache at daemon startup.
- **Worker pools** (embed, extract) pattern: `try_queue` drops on full, never
  blocks. Workers re-resolve config per task. Retry logic in `embed_worker.rs`
  (`retry_with_backoff`). 429 pause in `extract_worker.rs`.
- **ConflictClassifier** trait is defined in `storage::conflict_judge` (no
  external deps); implemented in `daemon::conflict_judge`.
- **sqlite-vec** must be registered before opening any DB that uses vec0:
  `pragmas::ensure_sqlite_vec_extension()`. Called in `db::open_write/read`.
- **Tests**: integration tests in `crates/memlayer-tests/` require a live
  daemon socket; they fail under sandbox-restricted envs. Unit tests in each
  crate's `#[cfg(test)] mod tests` run without a daemon.

## How to run

```bash
# Build all (including tests — catches compile errors in test code too)
cargo build --tests --workspace

# Unit tests for a specific module (avoids rebuilding rusqlite from scratch
# when backup.rs is sandbox-blocked — use the cached binary):
BIN=$(find target/debug/deps -maxdepth 1 -name 'memlayer_storage-*' -perm -u+x | tail -1)
$BIN history_chain --test-threads=1

# All unit tests for a crate (via cargo, requires rusqlite compile):
cargo test -p memlayer-storage --lib -- history_chain

# Run daemon in foreground (for debugging):
RUST_LOG=memlayer=debug cargo run -p memlayer-cli -- daemon start --foreground
```

## Config quick-reference (MemlayerConfig)

```toml
# ~/.memlayer/config.toml (global)
# ~/.memlayer/projects/<name>.config.toml (per-project overlay)

[extract]
enabled = false         # opt-in: Haiku/Sonnet fact decomposition on every save
model = "haiku"         # "haiku" | "sonnet"
timeout_secs = 30
workers = 1

[rerank]
model = "haiku"         # model used when --rerank is passed to obs search/context
timeout_secs = 5

[embed]
workers = 2
quantize = false        # store int8 BLOBs (384 B/obs) instead of float32 (1.5 KB/obs)

[conflict]
enabled = false         # LLM judge for supersession (replaces FTS5 title-match heuristic)
model = "haiku"
timeout_secs = 5
```

**Env overrides** (highest precedence, any session):
`MEMLAYER_EXTRACT_ENABLED`, `MEMLAYER_EXTRACT_MODEL`, `MEMLAYER_EXTRACT_TIMEOUT_SECS`,
`MEMLAYER_EXTRACT_WORKERS`, `MEMLAYER_RERANK_MODEL`, `MEMLAYER_RERANK_TIMEOUT_SECS`,
`MEMLAYER_EMBED_WORKERS`, `MEMLAYER_EMBED_QUANTIZE`,
`MEMLAYER_CONFLICT_ENABLED`, `MEMLAYER_CONFLICT_MODEL`, `MEMLAYER_CONFLICT_TIMEOUT_SECS`.

## Proto + gRPC surface (current RPCs)

Observation lifecycle: `SaveObservation`, `GetObservation`, `UpdateObservation`,
`DeleteObservation`, `SearchObservations` (supports `mode` + `rerank` fields),
`ListObservations`, `RecentObservations`.

Context / retrieval: `Context` (supports `mode`, `rerank`, `query`), `Timeline`,
`SuggestTopicKey`, `CapturePassive`.

Atomic facts: `GetFacts`, `GetObservationHistory`.

Bulk background ops: `ReextractObservations`, `ReindexObservations`.

Sessions: `StartSession`, `EndSession`, `SaveSessionSummary`, `GetSession`,
`ListSessions`, `DeleteSession`.

Prompts, Projects, Sync, Team/Admin, Health, Doctor RPCs — see `proto/memlayer.proto`.

## Pending roadmap (see `docs/ROADMAP.md`)

- Spec 2: MCP server (`memory/search`, `memory/add`, etc.)
- Spec 3: Code anchors + graphify bridge (`obs save --anchor file::symbol`)
- Spec 4 (shipped): LLM supersession judge — `conflict.enabled`
- Spec 5 (shipped): `obs history <id>` supersession chain
- Spec 1 deferred items (shipped): int8 quantize, cross-project hybrid,
  `obs reextract`, `memlayer reindex`
- `memlayer-eval` CI scorecard wiring (separate `eval-promotion` spec)
