//! Fixed Laya question schemas for StatefulMemory call sites.

use serde_json::{json, Value};

/// Router: `{easy, normal, hard}` choice.
pub fn router_questions() -> Value {
    json!({
        "tier": {
            "type": "choice",
            "options": ["easy", "normal", "hard"],
            "question": "How complex is this memory retrieval query for a coding-agent project memory store?"
        }
    })
}

pub fn router_state(query: &str) -> String {
    format!("Retrieve memory for coding agent.\nQuery: {query}")
}

/// Conflict relation labels (match [`crate::conflict_judge`] labels).
pub fn conflict_questions() -> Value {
    json!({
        "verdict": {
            "type": "choice",
            "options": ["SUPERSEDES", "CONFLICTS_WITH", "COMPATIBLE", "NOT_CONFLICT"],
            "question": "How does the NEWER observation relate to the OLDER one?"
        }
    })
}

pub fn conflict_state(old_title: &str, old_body: &str, new_title: &str, new_body: &str) -> String {
    let old_p: String = old_body.chars().take(600).collect();
    let new_p: String = new_body.chars().take(600).collect();
    format!(
        "OLDER:\nTitle: {old_title}\nContent: {old_p}\n\nNEWER:\nTitle: {new_title}\nContent: {new_p}"
    )
}

/// Resolve-pair 4-way action.
pub fn resolve_questions() -> Value {
    json!({
        "action": {
            "type": "choice",
            "options": ["keep_new", "keep_old", "keep_both", "synthesize"],
            "question": "How should these conflicting memories be resolved?"
        },
        "confidence": {
            "type": "score",
            "min": 0.0,
            "max": 1.0,
            "question": "Confidence in the chosen resolution action"
        }
    })
}

pub fn resolve_state(old_title: &str, old_body: &str, new_title: &str, new_body: &str) -> String {
    conflict_state(old_title, old_body, new_title, new_body)
}

/// Decide: pick among evidence titles + abstain; confidence + should_record.
pub fn decide_questions(options: &[String]) -> Value {
    let mut opts: Vec<String> = options.to_vec();
    if !opts.iter().any(|o| o == "abstain") {
        opts.push("abstain".into());
    }
    // Laya choice head budget — keep ≤ 20 options.
    if opts.len() > 20 {
        opts.truncate(19);
        opts.push("abstain".into());
    }
    json!({
        "recommendation": {
            "type": "choice",
            "options": opts,
            "question": "Which recommendation best answers the user question given the evidence?"
        },
        "confidence": {
            "type": "score",
            "min": 0.0,
            "max": 1.0,
            "question": "Confidence that the recommendation is correct"
        },
        "should_record": {
            "type": "noul",
            "question": "Should this decision be recorded as a resolution observation?"
        },
        "sufficient": {
            "type": "noul",
            "question": "Is the retrieved evidence sufficient to decide without further search?"
        }
    })
}

pub fn decide_state(question: &str, evidence_blob: &str, conflicts_blob: &str) -> String {
    format!(
        "Question: {question}\n\nEvidence:\n{evidence_blob}\n\nConflicts:\n{conflicts_blob}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_has_three_tiers() {
        let q = router_questions();
        let opts = q["tier"]["options"].as_array().unwrap();
        assert_eq!(opts.len(), 3);
    }

    #[test]
    fn decide_caps_options() {
        let many: Vec<String> = (0..30).map(|i| format!("opt-{i}")).collect();
        let q = decide_questions(&many);
        let opts = q["recommendation"]["options"].as_array().unwrap();
        assert!(opts.len() <= 20);
        assert_eq!(opts.last().unwrap().as_str().unwrap(), "abstain");
    }
}
