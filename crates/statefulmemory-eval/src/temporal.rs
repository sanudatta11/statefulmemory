//! Deterministic temporal constraints used by the eval retriever and tests.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalRelation {
    Before,
    After,
    Latest,
    First,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalConstraint {
    pub relation: TemporalRelation,
    pub year: Option<i32>,
}

pub fn detect(question: &str) -> TemporalConstraint {
    let q = question.to_ascii_lowercase();
    let relation = if q.contains("latest") || q.contains("most recent") || q.contains("last") {
        TemporalRelation::Latest
    } else if q.contains("first") || q.contains("earliest") {
        TemporalRelation::First
    } else if q.contains("before") || q.contains("earlier") {
        TemporalRelation::Before
    } else if q.contains("after") || q.contains("later") {
        TemporalRelation::After
    } else {
        TemporalRelation::Any
    };
    let year = q
        .split(|c: char| !c.is_ascii_digit())
        .find_map(|s| (s.len() == 4).then(|| s.parse().ok()).flatten());
    TemporalConstraint { relation, year }
}

pub fn score(constraint: &TemporalConstraint, hit: &str) -> i32 {
    let lower = hit.to_ascii_lowercase();
    if constraint
        .year
        .map(|y| !lower.contains(&y.to_string()))
        .unwrap_or(false)
    {
        return -100;
    }
    match constraint.relation {
        TemporalRelation::Any => 0,
        TemporalRelation::Before => {
            if lower.contains("before") || lower.contains("earlier") {
                2
            } else {
                0
            }
        }
        TemporalRelation::After => {
            if lower.contains("after") || lower.contains("later") {
                2
            } else {
                0
            }
        }
        TemporalRelation::Latest => {
            if lower.contains("latest") || lower.contains("most recent") {
                2
            } else {
                0
            }
        }
        TemporalRelation::First => {
            if lower.contains("first") || lower.contains("earliest") {
                2
            } else {
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_year_and_latest() {
        let c = detect("What was the latest event in 2023?");
        assert_eq!(c.relation, TemporalRelation::Latest);
        assert_eq!(c.year, Some(2023));
    }
    #[test]
    fn rejects_incompatible_year() {
        assert_eq!(
            score(&detect("What happened in 2023?"), "[2022] event"),
            -100
        );
    }
}
