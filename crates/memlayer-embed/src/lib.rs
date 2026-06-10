// Generated with AI Coding Rules Hub
//! memlayer-embed — local CPU embeddings (BGE-small) + on-disk cache.
//!
//! This crate provides:
//! * `Embedder` trait + `BgeSmallEmbedder` impl (spec-task-7)
//! * `EmbeddingCache` SQLite-backed cache keyed by sha256(text) (spec-task-6)
//! * `quantize` module with f32↔int8 helpers (spec-task-8 stubs, spec-task-24 activates)
//!
//! Runtime: candle-rs (pure Rust). Avoids fastembed/ort-sys precompiled binary
//! downloads that fail under enterprise TLS interception.
//!
//! See spec retrieval-upgrade-v1 §3.

pub mod cache;
pub mod embedder;
pub mod quantize;

pub use embedder::{BgeSmallEmbedder, Embedder};
