//! Re-export of [`statefulmemory_retrieval::rerank`] kept for backwards
//! compatibility with eval's runner. The authoritative implementation lives
//! in the shared `statefulmemory-retrieval` crate so the daemon (rp-t9) and eval
//! can use the same code path.

pub use statefulmemory_retrieval::rerank::*;
