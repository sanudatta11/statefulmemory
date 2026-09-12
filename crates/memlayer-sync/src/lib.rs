// Generated with AI Coding Rules Hub
//! `memlayer-sync`: git-chunk sync, JSON, and Markdown export/import.
//!
//! This crate is consumed by `memlayer-daemon` handlers.
//! The three public sub-modules expose the building blocks for Spec 3 (S2-S9):
//!
//! - `chunk`    — JSONL builder, SHA-256 chunk_id, zstd compress/decompress, atomic write
//! - `manifest` — `<repo>/.memlayer/manifest.json` read and atomic-append
//! - `error`    — `SyncError` and `Result<T>` alias

pub mod chunk;
pub mod error;
pub mod manifest;
pub mod mem_archive;
pub mod snapshot;

pub use error::{Result, SyncError};
