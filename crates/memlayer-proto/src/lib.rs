//! Re-exports of the tonic-generated stubs for `memlayer.v1`.
//!
//! Use as:
//! ```ignore
//! use memlayer_proto::{
//!     memlayer_server::Memlayer,
//!     SaveObservationRequest, SaveObservationResponse,
//! };
//! ```

#![allow(clippy::all)]
#![allow(unknown_lints)]

pub mod v1 {
    tonic::include_proto!("memlayer.v1");
}

// Convenience re-exports at crate root.
pub use v1::*;
pub use v1::memlayer_client;
pub use v1::memlayer_server;
