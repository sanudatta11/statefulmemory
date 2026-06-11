// Generated with AI Coding Rules Hub
//! memlayer-eval — accuracy and latency benchmarking against LoCoMo,
//! LongMemEval, and BEAM (1M / 10M scale).
//!
//! No daemon or Unix socket needed — evaluation calls the storage layer
//! directly via `ProjectRegistry` + `read::search`.

pub mod config;
pub mod datasets;
pub mod facts_db;
pub mod ingest;
pub mod judge;
pub mod prompt;
pub mod retrieve;
pub mod retrieve_hybrid;
pub mod rrf;
pub mod runner;
pub mod vec_index;

pub use runner::{BenchmarkKind, RunConfig, RunReport};
