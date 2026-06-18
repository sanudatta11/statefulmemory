//! Re-export of [`memlayer_retrieval::rerank`] kept for backwards
//! compatibility with eval's runner. The authoritative implementation lives
//! in the shared `memlayer-retrieval` crate so the daemon (rp-t9) and eval
//! can use the same code path.

pub use memlayer_retrieval::rerank::*;
