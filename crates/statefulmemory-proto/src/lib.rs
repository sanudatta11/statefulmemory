//! Re-exports of the tonic-generated stubs for `statefulmemory.v1`.
//!
//! Use as:
//! ```ignore
//! use statefulmemory_proto::{
//!     stateful_memory_server::StatefulMemory,
//!     SaveObservationRequest, SaveObservationResponse,
//! };
//! ```

#![allow(clippy::all)]
#![allow(unknown_lints)]

pub mod v1 {
    tonic::include_proto!("statefulmemory.v1");
}

// Convenience re-exports at crate root.
pub use v1::stateful_memory_client;
pub use v1::stateful_memory_server;
pub use v1::*;
