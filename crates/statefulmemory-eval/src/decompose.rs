//! Offline-safe multi-hop query decomposition helpers.

/// Parse a planner response into at most three subquestions. Invalid output
/// always falls back to the original query.
pub fn parse_subquestions(original: &str, planner_output: &str) -> Vec<String> {
    let mut out = planner_output
        .lines()
        .map(str::trim)
        .map(|s| {
            s.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | ' '))
                .trim()
        })
        .filter(|s| s.len() >= 8 && s.ends_with('?'))
        .map(ToOwned::to_owned)
        .take(3)
        .collect::<Vec<_>>();
    out.retain(|s| !s.eq_ignore_ascii_case(original.trim()));
    if out.is_empty() {
        vec![original.trim().to_string()]
    } else {
        out
    }
}

pub fn chain_score(
    subquestion_coverage: usize,
    entity_overlap: usize,
    temporal_compatible: bool,
    source_strength: usize,
) -> i32 {
    subquestion_coverage as i32 * 10
        + entity_overlap as i32 * 3
        + if temporal_compatible { 4 } else { -8 }
        + source_strength.min(5) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback() {
        assert_eq!(parse_subquestions("Who?", "not a plan"), vec!["Who?"]);
    }
    #[test]
    fn capped() {
        let x = parse_subquestions(
            "Q?",
            "1. Who is A?\n2. What does B like?\n3. Where is C?\n4. Extra question?",
        );
        assert_eq!(x.len(), 3);
        assert_eq!(x[0], "Who is A?");
    }
}
