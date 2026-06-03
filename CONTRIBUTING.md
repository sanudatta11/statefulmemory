# Contributing to memlayer

Thanks for helping improve memlayer. This guide covers the repository layout
and how to build and test locally.

## Repository layout

```
memlayer/
├── proto/memlayer.proto          # gRPC wire API
├── crates/
│   ├── memlayer-core/            # config, paths, error types
│   ├── memlayer-storage/         # SQLite + FTS5 + vec storage + write threads
│   ├── memlayer-embed/           # BGE-small embedder (candle-rs)
│   ├── memlayer-extract/         # Claude CLI fact extractor
│   ├── memlayer-retrieval/       # RRF, reranker, hybrid types
│   ├── memlayer-daemon/          # gRPC service + worker pools
│   ├── memlayer-client/          # channel builders (UDS + TCP)
│   ├── memlayer-cli/             # `memlayer` binary
│   ├── memlayer-sync/            # export / import
│   ├── memlayer-eval/            # benchmark harness (LoCoMo / LongMemEval)
│   └── memlayer-tests/           # integration tests
└── docs/PRD.md                   # authoritative product spec
```

## Build & test

```bash
cargo build --workspace --tests   # compile + test code
cargo test --workspace --lib      # unit tests (no live daemon)
cargo test -p memlayer-tests      # integration tests (spawns a temp daemon)
RUST_LOG=memlayer=debug cargo run -p memlayer-cli -- daemon start --foreground
```

CI runs the same build + unit + integration path on every push/PR to `main`
(see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

See [`CLAUDE.md`](CLAUDE.md) for architectural invariants, config reference,
and the gRPC surface listing.
