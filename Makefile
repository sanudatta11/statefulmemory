# memlayer — common development and user commands
# Run `make` or `make help` to see available targets.

.PHONY: help build release install test test-eval lint clean \
        daemon-start daemon-stop daemon-status logs skill-install \
        extract-locomo run-locomo

BINARY     := target/release/memlayer
INSTALL_DIR := $(HOME)/.local/bin

# ── Default ──────────────────────────────────────────────────────────────────

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Install"
	@echo "  install          Build release binary and symlink to $(INSTALL_DIR)/memlayer"
	@echo "  skill-install    Install agent skill globally + locally (Claude Code, Windsurf, Cursor, Copilot)"
	@echo ""
	@echo "Build"
	@echo "  build            Debug build (fast)"
	@echo "  release          Release build (optimised)"
	@echo ""
	@echo "Test"
	@echo "  test             Run full workspace test suite"
	@echo "  test-eval        Run memlayer-eval unit + integration tests"
	@echo ""
	@echo "Daemon"
	@echo "  daemon-start     Start daemon in background"
	@echo "  daemon-stop      Stop the running daemon"
	@echo "  daemon-status    Show daemon PID, uptime, socket path"
	@echo "  logs             Tail daemon log (last 50 lines)"
	@echo ""
	@echo "Eval"
	@echo "  extract-locomo   Extract facts for LoCoMo conv-26 (requires BGE model)"
	@echo "  run-locomo       Run LoCoMo benchmark (hybrid-rerank, k=20, limit=200)"
	@echo ""
	@echo "Misc"
	@echo "  lint             Run clippy on the workspace"
	@echo "  clean            cargo clean"

# ── Build ─────────────────────────────────────────────────────────────────────

build:
	cargo build -p memlayer-cli

release:
	cargo build --release -p memlayer-cli

# ── Install ───────────────────────────────────────────────────────────────────

install: release
	mkdir -p $(INSTALL_DIR)
	ln -sf $(PWD)/$(BINARY) $(INSTALL_DIR)/memlayer
	@echo "Installed: $(INSTALL_DIR)/memlayer"
	@echo "Make sure $(INSTALL_DIR) is on your PATH."

skill-install: install
	$(BINARY) install

# ── Test ──────────────────────────────────────────────────────────────────────

test:
	cargo test --workspace

test-eval:
	cargo test -p memlayer-eval --release

# ── Daemon ────────────────────────────────────────────────────────────────────

daemon-start:
	memlayer daemon start

daemon-stop:
	memlayer daemon stop

daemon-status:
	memlayer daemon status

logs:
	memlayer logs --lines 50

# ── Eval ──────────────────────────────────────────────────────────────────────

extract-locomo:
	@test -n "$(MEMLAYER_BGE_MODEL_DIR)" || (echo "Set MEMLAYER_BGE_MODEL_DIR first"; exit 1)
	cd crates/memlayer-eval && \
	RUST_LOG=info cargo run --release --bin eval -- extract \
	  --benchmark locomo --project locomo-conv-26 \
	  2>&1 | tee reports/extract-locomo.log

run-locomo:
	@test -n "$(MEMLAYER_BGE_MODEL_DIR)" || (echo "Set MEMLAYER_BGE_MODEL_DIR first"; exit 1)
	cd crates/memlayer-eval && \
	RUST_LOG=info cargo run --release --bin eval -- run \
	  --benchmark locomo --mode hybrid-rerank --evidence-window 2 \
	  --skip-ingest --limit 200 --k 20 \
	  --out reports/locomo.md \
	  2>&1 | tee reports/locomo.log

# ── Misc ──────────────────────────────────────────────────────────────────────

lint:
	cargo clippy --workspace -- -D warnings

clean:
	cargo clean
