// Generated with AI Coding Rules Hub
//! memlayer-eval — accuracy and latency benchmarking against LoCoMo,
//! LongMemEval, and BEAM (1M / 10M scale).
//!
//! No daemon or Unix socket needed — evaluation calls the storage layer
//! directly via `ProjectRegistry` + `read::search`.

pub mod config;
pub mod datasets;
pub mod ingest;
pub mod judge;
pub mod prompt;
pub mod retrieve;
pub mod runner;

pub use runner::{BenchmarkKind, RunConfig, RunReport};
