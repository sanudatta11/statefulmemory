//! memlayer daemon — library entry point.
//!
//! The binary in `main.rs` is a thin wrapper that parses the `daemon start`
//! / `daemon stop` subcommands and delegates to `lifecycle`.

pub mod admin_guard;
pub mod auth;
pub mod capture_passive;
pub mod conflict_judge;
pub mod decide;
pub mod embed_worker;
pub mod error_map;
pub mod extract_worker;
pub mod lifecycle;
pub mod logging;
pub mod server;
pub mod service;
pub mod signals;
pub mod suggest_topic_key;
pub mod mem_export;
pub mod mem_import;
pub mod resolve_worker;
pub mod verify;
pub mod verify_worker;
pub mod context_filter;
pub mod token_budget;
pub mod sync_export;
pub mod sync_status;
pub mod tls;
pub mod tokens;

pub use service::MemlayerService;
