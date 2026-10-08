//! Stable per-query failure diagnostics for offline regression analysis.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    RetrievalMiss,
    WrongTemporalVersion,
    IncompleteMultiHopChain,
    FactExtractionFailure,
    ContextPackingFailure,
    AnswerSynthesisFailure,
    JudgeProviderFailure,
}

/// Classify a completed query without calling a provider. Provider failures
/// are assigned by the runner at the error boundary; this function is for
/// reproducible retrieval/context regressions.
pub fn classify(
    category: Option<&str>,
    relevant_evidence: bool,
    fact_count: usize,
    observation_count: usize,
    answer_correct: bool,
) -> Option<FailureClass> {
    if answer_correct {
        return None;
    }
    if !relevant_evidence && fact_count == 0 && observation_count == 0 {
        return Some(FailureClass::RetrievalMiss);
    }
    if matches!(category, Some("temporal") | Some("2")) && relevant_evidence {
        return Some(FailureClass::WrongTemporalVersion);
    }
    if matches!(category, Some("multi_hop") | Some("1"))
        && relevant_evidence
        && (fact_count < 2 || observation_count < 2)
    {
        return Some(FailureClass::IncompleteMultiHopChain);
    }
    if fact_count == 0 {
        return Some(FailureClass::FactExtractionFailure);
    }
    if observation_count == 0 {
        return Some(FailureClass::ContextPackingFailure);
    }
    Some(FailureClass::AnswerSynthesisFailure)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_retrieval_miss() {
        assert_eq!(
            classify(Some("single_hop"), false, 0, 0, false),
            Some(FailureClass::RetrievalMiss)
        );
    }
    #[test]
    fn labels_temporal_version() {
        assert_eq!(
            classify(Some("temporal"), true, 1, 1, false),
            Some(FailureClass::WrongTemporalVersion)
        );
    }
    #[test]
    fn labels_multihop_chain() {
        assert_eq!(
            classify(Some("multi_hop"), true, 1, 1, false),
            Some(FailureClass::IncompleteMultiHopChain)
        );
    }
}
