//! Re-export of [`statefulmemory_retrieval::rrf`] kept for backwards compatibility
//! with eval-internal callers (`retrieve_hybrid::retrieve_hybrid` etc.). The
//! authoritative implementation lives in the shared `statefulmemory-retrieval`
//! crate so the daemon (rp-t8) and eval can use the same code path.

pub use statefulmemory_retrieval::rrf::*;
