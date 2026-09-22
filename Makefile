# statefulmemory — common development and user commands
# Run `make` or `make help` to see available targets.

.PHONY: help prereqs check-inputs build release install test test-eval lint clean \
        daemon-start daemon-stop daemon-status logs skill-install \
        extract-locomo run-locomo \
        eval-locomo-smoke eval-locomo-fetch eval-bge-model \
        eval-locomo-extract eval-locomo-facts \
        eval-locomo eval-locomo-full \
        eval-locomo-e2e eval-locomo-compare eval-staleness \
        laya-sidecar

export PATH := $(HOME)/.cargo/bin:$(PATH)

# Always build/run against the repo target/ (avoids stale sandbox CARGO_TARGET_DIR).
export CARGO_TARGET_DIR := $(CURDIR)/target

BINARY     := target/release/statefulmemory
INSTALL_DIR := $(HOME)/.local/bin
CARGO       := cargo

EVAL_DATA           := $(CURDIR)/data
EVAL_OUT            := $(CURDIR)/eval
LOCOMO_JSON         := $(EVAL_DATA)/locomo/locomo10.json
LOCOMO_URL          := https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json
SCORECARD_SMOKE     := $(EVAL_OUT)/locomo-smoke.json
SCORECARD_FULL      := $(EVAL_OUT)/locomo-full.json
LOCOMO_BASELINES    := $(CURDIR)/crates/statefulmemory-eval/baselines/locomo.json
COMPARE_LOCOMO      := python3 $(CURDIR)/tools/compare_locomo.py
COMPARE_STALENESS   := python3 $(CURDIR)/tools/compare_staleness.py
SCORECARD           ?= $(SCORECARD_FULL)
SCORECARD_STALE     := $(EVAL_OUT)/staleness.json
SCORECARD_STALE_BASE := $(EVAL_OUT)/staleness-baseline.json
BGE_MODEL_DIR       ?= $(HOME)/.statefulmemory-models/bge-small
VENDOR_BGE          := $(CURDIR)/crates/statefulmemory-eval/scripts/vendor_bge_model.sh

# Eval / hybrid paths need BGE without a manual shell export.
export STATEFULMEMORY_BGE_MODEL_DIR := $(BGE_MODEL_DIR)

# Optional capable pin for measure runs (disclosed on scorecard via STATEFULMEMORY_LLM_MODEL).
# Example: EVAL_MODEL=opencode/glm-5.3 make eval-locomo
ifdef EVAL_MODEL
export STATEFULMEMORY_LLM_MODEL := $(EVAL_MODEL)
endif

ifdef LIMIT
LIMIT_FLAG := --limit $(LIMIT)
else
LIMIT_FLAG :=
endif

# Optional pre-extract before full LoCoMo (builds data/locomo/facts.db).
# Default measure path wants facts: EXTRACT=1 unless explicitly EXTRACT=0.
# If facts.db already exists, extract is skipped (see eval-locomo recipe).
EXTRACT ?= 1
EXTRACT_DEPS :=
ifneq ($(filter 1 force,$(EXTRACT)),)
EXTRACT_DEPS := eval-locomo-facts
endif

.PHONY: eval-locomo-facts
# Build facts.db only when missing (or EXTRACT=force).
eval-locomo-facts: release eval-locomo-fetch eval-bge-model
	@if [ "$(EXTRACT)" = "force" ] || [ ! -f "$(EVAL_DATA)/locomo/facts.db" ]; then \
		$(MAKE) eval-locomo-extract; \
	else \
		echo "Using existing $(EVAL_DATA)/locomo/facts.db (EXTRACT=force to rebuild)"; \
	fi

# ── Default ──────────────────────────────────────────────────────────────────

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Prerequisites"
	@echo "  prereqs          Install Rust toolchain and required system dependencies"
	@echo "  check-inputs     Verify cargo, protoc, and embedded skill assets exist"
	@echo ""
	@echo "Install"
	@echo "  install          Build release binary and copy to $(INSTALL_DIR)/statefulmemory"
	@echo "  skill-install    Install agent skill globally + locally (Claude Code, Windsurf, Cursor, Copilot)"
	@echo ""
	@echo "Build"
	@echo "  build            Debug build (fast)"
	@echo "  release          Release build (optimised)"
	@echo ""
	@echo "Test"
	@echo "  test             Run full workspace test suite"
	@echo "  test-eval        Run statefulmemory-eval unit + integration tests"
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
	@echo "  eval-bge-model       Vendor BGE-small into BGE_MODEL_DIR (default ~/.statefulmemory-models/bge-small)"
	@echo "  eval-locomo-extract  Pre-extract facts.db for all LoCoMo projects (LLM + BGE)"
	@echo "  eval-locomo          Full locomo10: hybrid-rerank + LLM answer/judge"
	@echo "  eval-locomo-full     Alias for eval-locomo"
	@echo "  eval-locomo-e2e      Smoke then full"
	@echo "  eval-locomo-compare  Print SCORECARD vs published paper/LLM-judge bands"
	@echo "  eval-staleness       Fixture: supersession vs --no-supersede baseline"
	@echo "                       LIMIT=N = stratified cats 1–4; EXTRACT=1 (default) builds facts.db if missing"
	@echo "                       STATEFULMEMORY_EVAL_CONCURRENCY=N parallel answer/judge (default 4)"
	@echo ""
	@echo "Laya (Wave 4 System-1)"
	@echo "  laya-sidecar         Start Laya HTTP sidecar on 127.0.0.1:8765"
	@echo ""
	@echo "Eval (legacy eval binary in crates/statefulmemory-eval)"
	@echo "  extract-locomo   Extract facts for LoCoMo conv-26 (requires BGE model)"
	@echo "  run-locomo       Run LoCoMo benchmark (hybrid-rerank, k=20, limit=200)"
	@echo ""
	@echo "Misc"
	@echo "  lint             Run clippy on the workspace"
	@echo "  clean            cargo clean"

# ── Prerequisites ─────────────────────────────────────────────────────────────

prereqs:
	@echo "Checking and installing prerequisites for statefulmemory..."
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
	@test -f skills/statefulmemory/SKILL.md || (echo "error: skills/statefulmemory/SKILL.md missing (required — embedded into the CLI)."; exit 1)
	@test -f proto/statefulmemory.proto || (echo "error: proto/statefulmemory.proto missing."; exit 1)

# ── Build ─────────────────────────────────────────────────────────────────────

build: check-inputs
	cargo build -p statefulmemory-cli

release: check-inputs
	cargo build --release -p statefulmemory-cli

# ── Install ───────────────────────────────────────────────────────────────────

# Copy (not symlink) so the install survives moving/deleting the source tree.
install: release
	mkdir -p $(INSTALL_DIR)
	install -m 755 $(BINARY) $(INSTALL_DIR)/statefulmemory
	install -m 755 $(BINARY) $(INSTALL_DIR)/smem
	@echo "Installed: $(INSTALL_DIR)/statefulmemory"
	@echo "Installed: $(INSTALL_DIR)/smem  (shorthand)"
	@$(INSTALL_DIR)/smem --version
	@echo "Make sure $(INSTALL_DIR) is on your PATH."
skill-install: install
	$(INSTALL_DIR)/smem install

# ── Test ──────────────────────────────────────────────────────────────────────

test:
	cargo test --workspace

test-eval:
	cargo test -p statefulmemory-eval --release

# ── Daemon ────────────────────────────────────────────────────────────────────

daemon-start:
	smem daemon start

daemon-stop:
	smem daemon stop

daemon-status:
	smem daemon status

logs:
	smem logs --lines 50

# ── Eval ──────────────────────────────────────────────────────────────────────

eval-locomo-smoke: release
	@mkdir -p "$(EVAL_OUT)"
	STATEFULMEMORY_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --smoke --benchmark locomo \
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
# Session-tier summary facts are on by default (Mem0-style rollups).
eval-locomo-extract: release eval-locomo-fetch eval-bge-model
	@mkdir -p "$(EVAL_OUT)"
	@echo "Extracting LoCoMo facts → $(EVAL_DATA)/locomo/facts.db (needs agent CLI; session summaries on)."
	cd crates/statefulmemory-eval && \
	RUST_LOG=info cargo run --release --bin eval -- extract \
	  --benchmark locomo --data-dir "$(EVAL_DATA)" --session-summaries \
	  2>&1 | tee "$(EVAL_OUT)/extract-locomo.log"

eval-locomo eval-locomo-full: release eval-locomo-fetch eval-bge-model $(EXTRACT_DEPS)
	@mkdir -p "$(EVAL_OUT)"
	@echo "Full LoCoMo: STATEFULMEMORY_BGE_MODEL_DIR=$(STATEFULMEMORY_BGE_MODEL_DIR) model=$${STATEFULMEMORY_LLM_MODEL:-default} (hybrid+rerank; needs agent CLI for answer/judge)."
	STATEFULMEMORY_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --benchmark locomo $(LIMIT_FLAG) \
	  --save-scorecard "$(SCORECARD_FULL)"
	@$(COMPARE_LOCOMO) --scorecard "$(SCORECARD_FULL)" --baselines "$(LOCOMO_BASELINES)"

eval-locomo-e2e: eval-locomo-smoke eval-locomo-full

eval-locomo-compare:
	@test -f "$(SCORECARD)" || (echo "error: SCORECARD not found: $(SCORECARD)"; exit 1)
	$(COMPARE_LOCOMO) --scorecard "$(SCORECARD)" --baselines "$(LOCOMO_BASELINES)"

eval-staleness: release
	@mkdir -p "$(EVAL_OUT)"
	STATEFULMEMORY_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --smoke --benchmark staleness \
	  --save-scorecard "$(SCORECARD_STALE)"
	STATEFULMEMORY_EVAL_DATA="$(EVAL_DATA)" "$(BINARY)" eval --smoke --benchmark staleness \
	  --no-supersede --save-scorecard "$(SCORECARD_STALE_BASE)"
	@$(COMPARE_STALENESS) --scorecard "$(SCORECARD_STALE)" --baseline "$(SCORECARD_STALE_BASE)"

extract-locomo: eval-bge-model
	cd crates/statefulmemory-eval && \
	RUST_LOG=info cargo run --release --bin eval -- extract \
	  --benchmark locomo --project locomo-conv-26 \
	  2>&1 | tee reports/extract-locomo.log

run-locomo: eval-bge-model
	cd crates/statefulmemory-eval && \
	RUST_LOG=info cargo run --release --bin eval -- run \
	  --benchmark locomo --mode hybrid-rerank --evidence-window 2 \
	  --skip-ingest --limit 200 --k 20 \
	  --out reports/locomo.md \
	  2>&1 | tee reports/locomo.log

# ── Laya System-1 sidecar (Wave 4) ────────────────────────────────────────────

laya-sidecar:
	@echo "Starting Laya sidecar on 127.0.0.1:8765 (USE_TF=0)"
	cd $(CURDIR)/tools/laya-sidecar && \
	  USE_TF=0 LAYA_HOST=127.0.0.1 LAYA_PORT=8765 \
	  python3 -m uvicorn server:app --host 127.0.0.1 --port 8765

# ── Misc ──────────────────────────────────────────────────────────────────────

lint:
	cargo clippy --workspace -- -D warnings

clean:
	cargo clean
