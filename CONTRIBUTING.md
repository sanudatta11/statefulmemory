# Contributing to statefulmemory

Thanks for helping improve statefulmemory. This guide covers the repository layout
and how to build and test locally.

## Repository layout

```
statefulmemory/
├── proto/statefulmemory.proto          # gRPC wire API
├── crates/
│   ├── statefulmemory-core/            # config, paths, error types
│   ├── statefulmemory-storage/         # SQLite + FTS5 + vec storage + write threads
│   ├── statefulmemory-embed/           # BGE-small embedder (candle-rs)
│   ├── statefulmemory-extract/         # Claude CLI fact extractor
│   ├── statefulmemory-retrieval/       # RRF, reranker, hybrid types
│   ├── statefulmemory-daemon/          # gRPC service + worker pools
│   ├── statefulmemory-client/          # channel builders (UDS + TCP)
│   ├── statefulmemory-cli/             # `statefulmemory` binary
│   ├── statefulmemory-sync/            # export / import
│   ├── statefulmemory-eval/            # benchmark harness (LoCoMo / LongMemEval)
│   └── statefulmemory-tests/           # integration tests
└── docs/PRD.md                   # authoritative product spec
```

## Build & test

```bash
cargo build --workspace --tests   # compile + test code
cargo test --workspace --lib      # unit tests (no live daemon)
cargo test -p statefulmemory-tests      # integration tests (spawns a temp daemon)
RUST_LOG=statefulmemory=debug cargo run -p statefulmemory-cli -- daemon start --foreground
```

CI runs the same build + unit + integration path on every push/PR to `main`
(see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

## Git commit hooks

Enable the repo hooks once per clone (sets `core.hooksPath`):

```bash
git config core.hooksPath .githooks
```

`.githooks/commit-msg` requires a **single-line** Conventional Commits subject
and rejects `Co-authored-by` / `Assisted-by` / `Signed-off-by` and similar AI
attribution. See [`.cursor/rules/git-commits.mdc`](.cursor/rules/git-commits.mdc).

See [`CLAUDE.md`](CLAUDE.md) for architectural invariants, config reference,
and the gRPC surface listing.

## License

Contributions are dual-licensed under MIT OR Apache-2.0 (same as the project).
See [`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), and the
License section in [`README.md`](README.md).
