// Generated with AI Coding Rules Hub
//! memlayer-eval — accuracy and latency benchmarking against LoCoMo,
//! LongMemEval, and BEAM (1M / 10M scale).
//!
//! No daemon or Unix socket needed — evaluation calls the storage layer
//! directly via `ProjectRegistry` + `read::search`.

pub mod config;
pub mod datasets;
pub mod entities_writer;
pub mod entity_walk;
pub mod extract_pipeline;
pub mod facts_db;
pub mod facts_writer;
pub mod ingest;
pub mod judge;
pub mod prompt;
pub mod retrieve;
pub mod retrieve_facts;
pub mod retrieve_hybrid;
pub mod rerank;
pub mod rrf;
pub mod runner;
pub mod scoring;
pub mod vec_index;

pub use runner::{BenchmarkKind, RunConfig, RunReport};
