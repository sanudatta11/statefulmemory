# memlayer — common development and user commands
# Run `make` or `make help` to see available targets.

.PHONY: help prereqs check-inputs build release install test test-eval lint clean \
        daemon-start daemon-stop daemon-status logs skill-install \
        extract-locomo run-locomo \
        eval-locomo-smoke eval-locomo-fetch eval-bge-model \
        eval-locomo-extract \
        eval-locomo eval-locomo-full \
        eval-locomo-e2e eval-locomo-compare eval-staleness

export PATH := $(HOME)/.cargo/bin:$(PATH)

BINARY     := target/release/memlayer
INSTALL_DIR := $(HOME)/.local/bin
CARGO       := cargo

EVAL_DATA           := $(CURDIR)/data
EVAL_OUT            := $(CURDIR)/eval
LOCOMO_JSON         := $(EVAL_DATA)/locomo/locomo10.json
LOCOMO_URL          := https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json
SCORECARD_SMOKE     := $(EVAL_OUT)/locomo-smoke.json
SCORECARD_FULL      := $(EVAL_OUT)/locomo-full.json
LOCOMO_BASELINES    := $(CURDIR)/crates/memlayer-eval/baselines/locomo.json
COMPARE_LOCOMO      := python3 $(CURDIR)/tools/compare_locomo.py
COMPARE_STALENESS   := python3 $(CURDIR)/tools/compare_staleness.py
SCORECARD           ?= $(SCORECARD_FULL)
SCORECARD_STALE     := $(EVAL_OUT)/staleness.json
SCORECARD_STALE_BASE := $(EVAL_OUT)/staleness-baseline.json
BGE_MODEL_DIR       ?= $(HOME)/.memlayer-models/bge-small
VENDOR_BGE          := $(CURDIR)/crates/memlayer-eval/scripts/vendor_bge_model.sh

# Eval / hybrid paths need BGE without a manual shell export.
export MEMLAYER_BGE_MODEL_DIR := $(BGE_MODEL_DIR)

ifdef LIMIT
LIMIT_FLAG := --limit $(LIMIT)
else
LIMIT_FLAG :=
endif

# Optional pre-extract before full LoCoMo (builds data/locomo/facts.db).
EXTRACT_DEPS :=
ifeq ($(EXTRACT),1)
EXTRACT_DEPS := eval-locomo-extract
endif

# ── Default ──────────────────────────────────────────────────────────────────

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Prerequisites"
	@echo "  prereqs          Install Rust toolchain and required system dependencies"
	@echo "  check-inputs     Verify cargo, protoc, and embedded skill assets exist"
	@echo ""
	@echo "Install"
	@echo "  install          Build release binary and copy to $(INSTALL_DIR)/memlayer"
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
	@echo "Eval (LoCoMo e2e — preferred for local analysis)"
	@echo "  eval-locomo-smoke    Fixture: BM25 + lexical judge, write eval/locomo-smoke.json"
	@echo "  eval-locomo-fetch    Download SNAP locomo10.json into data/locomo/"
	@echo "  eval-bge-model       Vendor BGE-small into BGE_MODEL_DIR (default ~/.memlayer-models/bge-small)"
	@echo "  eval-locomo-extract  Pre-extract facts.db for all LoCoMo projects (LLM + BGE)"
	@echo "  eval-locomo          Full locomo10: hybrid-rerank + LLM answer/judge"
	@echo "  eval-locomo-full     Alias for eval-locomo"
	@echo "  eval-locomo-e2e      Smoke then full"
	@echo "  eval-locomo-compare  Print SCORECARD vs published paper/LLM-judge bands"
	@echo "  eval-staleness       Fixture: supersession vs --no-supersede baseline"
	@echo "                       LIMIT=N slices the full run; EXTRACT=1 runs extract first"
	@echo "                       MEMLAYER_EVAL_CONCURRENCY=N parallel answer/judge (default 4)"
	@echo ""
	@echo "Eval (legacy eval binary in crates/memlayer-eval)"
	@echo "  extract-locomo   Extract facts for LoCoMo conv-26 (requires BGE model)"
	@echo "  run-locomo       Run LoCoMo benchmark (hybrid-rerank, k=20, limit=200)"
	@echo ""
	@echo "Misc"
	@echo "  lint             Run clippy on the workspace"
	@echo "  clean            cargo clean"

# ── Prerequisites ─────────────────────────────────────────────────────────────

prereqs:
	@echo "Checking and installing prerequisites for memlayer..."
	@if command -v apt-get >/dev/null 2>&1; then \
		echo "Detected apt package manager. Installing system dependencies..."; \
		sudo apt-get update && sudo apt-get install -y build-essential curl git pkg-config libssl-dev protobuf-compiler; \
	elif command -v brew >/dev/null 2>&1; then \
		echo "Detected Homebrew. Installing system dependencies..."; \
		brew install protobuf pkg-config openssl git curl; \
	else \
		echo "Package manager not automatically detected. Ensure build-essential, pkg-config, protobuf-compiler, curl, git are installed."; \
	fi
	@if ! command -v rustup >/dev/null 2>&1 && ! command -v cargo >/dev/null 2>&1; then \
		echo "Rust not found. Installing Rust via rustup..."; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable; \
		. "$$HOME/.cargo/env"; \
	else \
		echo "Rust is already installed: $$(cargo --version)"; \
	fi
	@if command -v rustup >/dev/null 2>&1; then \
		echo "Ensuring required Rust toolchain components (rustfmt, clippy)..."; \
		rustup component add rustfmt clippy; \
	fi
	@command -v protoc >/dev/null || (echo "error: protoc still missing after prereqs"; exit 1)
	@echo "protoc: $$(protoc --version)"
	@echo "Prerequisites check complete!"

# Fail fast with actionable errors before a long release compile.
check-inputs:
	@command -v cargo >/dev/null || (echo "error: cargo not found. Run 'make prereqs'."; exit 1)
	@command -v protoc >/dev/null || (echo "error: protoc not found. Run 'make prereqs' (needs protobuf-compiler)."; exit 1)
	@test -f skills/memlayer/SKILL.md || (echo "error: skills/memlayer/SKILL.md missing (required — embedded into the CLI)."; exit 1)
	@test -f proto/memlayer.proto || (echo "error: proto/memlayer.proto missing."; exit 1)

# ── Build ─────────────────────────────────────────────────────────────────────

build: check-inputs
	cargo build -p memlayer-cli

release: check-inputs
	cargo build --release -p memlayer-cli

# ── Install ───────────────────────────────────────────────────────────────────

# Copy (not symlink) so the install survives moving/deleting the source tree.
install: release
	mkdir -p $(INSTALL_DIR)
	install -m 755 $(BINARY) $(INSTALL_DIR)/memlayer
	@echo "Installed: $(INSTALL_DIR)/memlayer"
	@$(INSTALL_DIR)/memlayer --version
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

eval-locomo-smoke: release
	@mkdir -p "$(EVAL_OUT)"
	MEMLAYER_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --smoke --benchmark locomo \
	  --save-scorecard "$(SCORECARD_SMOKE)"
	@$(COMPARE_LOCOMO) --scorecard "$(SCORECARD_SMOKE)" --baselines "$(LOCOMO_BASELINES)"

eval-locomo-fetch:
	@mkdir -p "$(EVAL_DATA)/locomo"
	@if [ -f "$(LOCOMO_JSON)" ]; then \
		echo "Already present: $(LOCOMO_JSON)"; \
	else \
		echo "Fetching $(LOCOMO_URL)"; \
		curl -fL --retry 4 --retry-delay 2 -o "$(LOCOMO_JSON)" "$(LOCOMO_URL)"; \
		echo "Wrote $(LOCOMO_JSON)"; \
	fi

eval-bge-model:
	@QUIET=1 "$(VENDOR_BGE)"

# Pre-extract facts for every LoCoMo conversation (expensive; opt-in via EXTRACT=1).
eval-locomo-extract: release eval-locomo-fetch eval-bge-model
	@mkdir -p "$(EVAL_OUT)"
	@echo "Extracting LoCoMo facts → $(EVAL_DATA)/locomo/facts.db (needs agent CLI)."
	cd crates/memlayer-eval && \
	RUST_LOG=info cargo run --release --bin eval -- extract \
	  --benchmark locomo --data-dir "$(EVAL_DATA)" \
	  2>&1 | tee "$(EVAL_OUT)/extract-locomo.log"

eval-locomo eval-locomo-full: release eval-locomo-fetch eval-bge-model $(EXTRACT_DEPS)
	@mkdir -p "$(EVAL_OUT)"
	@echo "Full LoCoMo: MEMLAYER_BGE_MODEL_DIR=$(MEMLAYER_BGE_MODEL_DIR) (hybrid+rerank; needs agent CLI for answer/judge)."
	MEMLAYER_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --benchmark locomo $(LIMIT_FLAG) \
	  --save-scorecard "$(SCORECARD_FULL)"
	@$(COMPARE_LOCOMO) --scorecard "$(SCORECARD_FULL)" --baselines "$(LOCOMO_BASELINES)"

eval-locomo-e2e: eval-locomo-smoke eval-locomo-full

eval-locomo-compare:
	@test -f "$(SCORECARD)" || (echo "error: SCORECARD not found: $(SCORECARD)"; exit 1)
	$(COMPARE_LOCOMO) --scorecard "$(SCORECARD)" --baselines "$(LOCOMO_BASELINES)"

eval-staleness: release
	@mkdir -p "$(EVAL_OUT)"
	MEMLAYER_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --smoke --benchmark staleness \
	  --save-scorecard "$(SCORECARD_STALE)"
	MEMLAYER_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --smoke --benchmark staleness \
	  --no-supersede --save-scorecard "$(SCORECARD_STALE_BASE)"
	@$(COMPARE_STALENESS) --scorecard "$(SCORECARD_STALE)" --baseline "$(SCORECARD_STALE_BASE)"

extract-locomo: eval-bge-model
	cd crates/memlayer-eval && \
	RUST_LOG=info cargo run --release --bin eval -- extract \
	  --benchmark locomo --project locomo-conv-26 \
	  2>&1 | tee reports/extract-locomo.log

run-locomo: eval-bge-model
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
