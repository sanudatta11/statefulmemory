//! Trait for LLM-based conflict / supersession classification (Spec 4).
//!
//! Defined here so `memlayer-storage` stays pure (no LLM dep); the
//! implementation lives in `memlayer-daemon/src/conflict_judge.rs` and is
//! injected into the write thread at daemon startup.
//!
//! When the configured judge returns an error (timeout, model unavailable,
//! parse failure) the write path falls back to the existing BM25-title-match
//! heuristic — unconditional supersession — and logs a warning.

use anyhow::Result;

/// Four-way relation between an older and a newer observation with the same
/// `(type, scope)` and a similar title (BM25 candidate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictVerdict {
    /// The new observation replaces the old (delete old).
    Supersedes,
    /// The two observations are mutually incompatible (delete old).
    ConflictsWith,
    /// The new observation is a complementary addition; keep both.
    Compatible,
    /// The observations are about different things despite similar titles; keep both.
    NotConflict,
}

/// Pluggable conflict classifier. Implement on a struct that holds a
/// `ClaudeClient` (in the daemon crate) and pass it into `spawn_write_thread`
/// at registry open time. The trait is object-safe so it can be stored as
/// `Arc<dyn ConflictClassifier>`.
pub trait ConflictClassifier: Send + Sync {
    /// Classify the relationship between an older and a newer observation.
    ///
    /// Implementations MUST:
    /// - Return `Ok(verdict)` when they have a confident classification.
    /// - Return `Err` on timeout, transport failure, or ambiguous response.
    ///   The caller falls back to heuristic supersession on any error.
    fn classify(
        &self,
        old_title: &str,
        old_content: &str,
        new_title: &str,
        new_content: &str,
    ) -> Result<ConflictVerdict>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockClassifier(ConflictVerdict);

    impl ConflictClassifier for MockClassifier {
        fn classify(&self, _: &str, _: &str, _: &str, _: &str) -> Result<ConflictVerdict> {
            Ok(self.0)
        }
    }

    struct ErrorClassifier;

    impl ConflictClassifier for ErrorClassifier {
        fn classify(&self, _: &str, _: &str, _: &str, _: &str) -> Result<ConflictVerdict> {
            Err(anyhow::anyhow!("simulated LLM failure"))
        }
    }

    #[test]
    fn mock_supersedes() {
        let c = MockClassifier(ConflictVerdict::Supersedes);
        assert_eq!(
            c.classify("old", "body", "new", "body").unwrap(),
            ConflictVerdict::Supersedes
        );
    }

    #[test]
    fn mock_not_conflict() {
        let c = MockClassifier(ConflictVerdict::NotConflict);
        assert_eq!(
            c.classify("old", "body", "new", "body").unwrap(),
            ConflictVerdict::NotConflict
        );
    }

    #[test]
    fn error_classifier_propagates() {
        let c = ErrorClassifier;
        assert!(c.classify("old", "body", "new", "body").is_err());
    }
}
