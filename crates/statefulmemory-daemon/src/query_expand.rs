//! LongMemEval-inspired query expansion for retrieve / compile.
//!
//! * **Fact-augmented keys (merge mode)** — append distinct fact subjects /
//!   objects from a cheap FTS pass so BM25/dense see the same cues the fact
//!   index already knows.
//! * **Time-aware pruning** — heuristic (no LLM) time-range inference for
//!   phrases like `last week`, `yesterday`, `in 2024`; soft-filters hits
//!   outside the window when enough in-range candidates remain.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use statefulmemory_storage::facts as facts_q;
use statefulmemory_storage::Observation;

/// Expand `query` by merging top fact subject/object tokens (LongMemEval
/// `session-userfact` / `merge` join mode, heuristics-only).
pub fn expand_query_with_facts(conn: &rusqlite::Connection, query: &str, limit: i64) -> String {
    let q = query.trim();
    if q.is_empty() {
        return String::new();
    }
    let Ok(facts) = facts_q::search_facts(conn, q, limit.max(1)) else {
        return q.to_string();
    };
    if facts.is_empty() {
        return q.to_string();
    }
    let mut seen = HashSet::new();
    let mut extras: Vec<String> = Vec::new();
    for f in facts {
        for term in [f.subject.as_str(), f.object.as_str()] {
            let t = term.trim();
            if t.is_empty() || t.len() < 2 {
                continue;
            }
            // Skip tokens already present in the raw query (case-insensitive).
            if q.to_ascii_lowercase().contains(&t.to_ascii_lowercase()) {
                continue;
            }
            if seen.insert(t.to_ascii_lowercase()) {
                extras.push(t.to_string());
            }
            if extras.len() >= 8 {
                break;
            }
        }
        if extras.len() >= 8 {
            break;
        }
    }
    if extras.is_empty() {
        return q.to_string();
    }
    format!("{q} {}", extras.join(" "))
}

/// Inclusive `[start, end]` UTC window inferred from the query text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Heuristic time-range extractor (LongMemEval time-aware query expansion
/// without an LLM). Returns `None` when the query looks non-temporal.
pub fn infer_time_range(query: &str, now: DateTime<Utc>) -> Option<TimeRange> {
    let lower = query.to_ascii_lowercase();
    let has_temporal = [
        "yesterday",
        "last week",
        "last weekend",
        "last month",
        "last year",
        "today",
        "this week",
        "this month",
        "ago",
        "in 20",
        "before ",
        "after ",
        "on 20",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if !has_temporal {
        // Bare ISO date mention.
        extract_iso_date(&lower)?;
    }

    if lower.contains("yesterday") {
        let day = (now - Duration::days(1)).date_naive();
        return Some(day_range(day));
    }
    if lower.contains("today") {
        return Some(day_range(now.date_naive()));
    }
    if lower.contains("last weekend") {
        // Previous Sat–Sun relative to `now`.
        let weekday = now.weekday().num_days_from_monday() as i64;
        // Days since last Sunday end: weekday+1 (Mon=0 → 1 day since Sun).
        let since_sun = weekday + 1;
        let last_sun = (now - Duration::days(since_sun)).date_naive();
        let last_sat = last_sun - Duration::days(1);
        return Some(TimeRange {
            start: start_of_day(last_sat),
            end: end_of_day(last_sun),
        });
    }
    if lower.contains("last week") || lower.contains("this week") {
        let days = if lower.contains("this week") { 7 } else { 14 };
        return Some(TimeRange {
            start: now - Duration::days(days),
            end: now,
        });
    }
    if lower.contains("last month") || lower.contains("this month") {
        let days = if lower.contains("this month") { 31 } else { 62 };
        return Some(TimeRange {
            start: now - Duration::days(days),
            end: now,
        });
    }
    if lower.contains("last year") {
        return Some(TimeRange {
            start: now - Duration::days(365 * 2),
            end: now,
        });
    }
    if let Some(d) = extract_iso_date(&lower) {
        return Some(day_range(d));
    }
    // Fallback: recent 90 days when temporal cue present but unparsed.
    Some(TimeRange {
        start: now - Duration::days(90),
        end: now,
    })
}

/// Soft time prune: keep in-range hits; if fewer than `min_keep`, return the
/// original list unchanged (recall over precision).
pub fn soft_filter_by_time(
    hits: Vec<Observation>,
    range: &TimeRange,
    min_keep: usize,
) -> Vec<Observation> {
    if hits.is_empty() {
        return hits;
    }
    let in_range: Vec<Observation> = hits
        .iter()
        .filter(|o| obs_in_range(o, range))
        .cloned()
        .collect();
    if in_range.len() >= min_keep {
        in_range
    } else {
        hits
    }
}

fn obs_in_range(o: &Observation, range: &TimeRange) -> bool {
    let ts = parse_obs_ts(&o.created_at).or_else(|| parse_obs_ts(&o.updated_at));
    match ts {
        Some(t) => t >= range.start && t <= range.end,
        None => true, // keep unparseable rather than drop
    }
}

fn parse_obs_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
        .or_else(|| {
            NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(12, 0, 0))
                .map(|ndt| DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
        })
}

fn extract_iso_date(lower: &str) -> Option<NaiveDate> {
    // Find YYYY-MM-DD
    let bytes = lower.as_bytes();
    for i in 0..bytes.len().saturating_sub(9) {
        let slice = &lower[i..i + 10];
        if let Ok(d) = NaiveDate::parse_from_str(slice, "%Y-%m-%d") {
            return Some(d);
        }
    }
    // Find "in 2024" / "in 2025"
    if let Some(idx) = lower.find("in 20") {
        let rest = &lower[idx + 3..];
        if rest.len() >= 4 {
            if let Ok(y) = rest[..4].parse::<i32>() {
                return NaiveDate::from_ymd_opt(y, 1, 1);
            }
        }
    }
    None
}

fn day_range(day: NaiveDate) -> TimeRange {
    TimeRange {
        start: start_of_day(day),
        end: end_of_day(day),
    }
}

fn start_of_day(day: NaiveDate) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(day.and_hms_opt(0, 0, 0).unwrap(), Utc)
}

fn end_of_day(day: NaiveDate) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(day.and_hms_opt(23, 59, 59).unwrap(), Utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yesterday_range() {
        let now = DateTime::parse_from_rfc3339("2026-09-22T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let r = infer_time_range("what did we decide yesterday?", now).unwrap();
        assert_eq!(r.start.date_naive().to_string(), "2026-09-21");
    }

    #[test]
    fn non_temporal_is_none() {
        let now = Utc::now();
        assert!(infer_time_range("how does hybrid search work?", now).is_none());
    }

    #[test]
    fn soft_filter_keeps_original_when_sparse() {
        let range = TimeRange {
            start: Utc::now() - Duration::days(1),
            end: Utc::now(),
        };
        let hits = vec![Observation {
            id: 1,
            sync_id: "s".into(),
            session_id: "sess".into(),
            r#type: "note".into(),
            title: "old".into(),
            content: "x".into(),
            tool_name: None,
            scope: "project".into(),
            created_by: None,
            topic_key: None,
            normalized_hash: None,
            revision_count: 1,
            duplicate_count: 1,
            last_seen_at: None,
            created_at: "2020-01-01T00:00:00Z".into(),
            updated_at: "2020-01-01T00:00:00Z".into(),
            deleted_at: None,
            review_after: None,
            code_anchor: None,
            superseded_count: 0,
            superseded_ids: vec![],
            verify_state: "unanchored".into(),
        }];
        let out = soft_filter_by_time(hits.clone(), &range, 1);
        assert_eq!(out.len(), 1); // fell back
        assert_eq!(out[0].id, 1);
    }
}
