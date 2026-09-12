//! `memlayer-retrieval` — shared retrieval primitives.
//!
//! This crate exists so the daemon (`/catalyst-spec retrieval-promotion` rp-t8,
//! rp-t9) and the eval harness (`memlayer-eval`) can share one implementation
//! of:
//!
//! * **`rrf`** — Reciprocal Rank Fusion for merging BM25 + dense candidate lists.
//! * **`rerank`** — LLM rerank that reorders top-N candidates via a Claude
//!   shell-out (Haiku or Sonnet selectable).
//! * **`hybrid`** — common types used by hybrid-retrieval callers (mode flags,
//!   fused result rows). The actual hybrid orchestration lives at the call
//!   site (eval has its own sidecar-DB version, the daemon has a per-project
//!   version).
//!
//! Pure file-move from `memlayer-eval` plus thin shared types. `memlayer-eval`
//! re-exports `rrf` and `rerank` so its existing callers keep working.

pub mod facts_fuse;
pub mod hybrid;
pub mod rerank;
pub mod rrf;
