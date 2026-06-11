// Generated with AI Coding Rules Hub
//! Skeleton TS-6: parse_facts handles realistic Haiku response shapes.

use memlayer_extract::{parse_facts, Turn};

fn turns_2() -> Vec<Turn> {
    vec![
        Turn {
            speaker: "Caroline".into(),
            text: "I went to the LGBTQ support group on May 8.".into(),
            obs_id: 101,
            session_id: Some("s-abc".into()),
        },
        Turn {
            speaker: "Melanie".into(),
            text: "How was it?".into(),
            obs_id: 102,
            session_id: Some("s-abc".into()),
        },
    ]
}

#[test]
fn ts6_clean_json_array_parses() {
    let raw = r#"[
      {"subject": "Caroline", "predicate": "attended", "object": "LGBTQ support group",
       "temporal": "2023-05-08", "salience": 0.9, "evidence_turn_idx": 1}
    ]"#;
    let facts = parse_facts(raw, &turns_2()).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].subject, "Caroline");
    assert_eq!(facts[0].evidence_obs_id, 101);
    assert_eq!(facts[0].source_session.as_deref(), Some("s-abc"));
}

#[test]
fn ts6_leading_prose_is_stripped() {
    let raw = r#"Sure, here are the facts:
    [
      {"subject": "Caroline", "predicate": "attended", "object": "support group",
       "salience": 0.8, "evidence_turn_idx": 1}
    ]
    Hope that helps!"#;
    let facts = parse_facts(raw, &turns_2()).unwrap();
    assert_eq!(facts.len(), 1);
    assert!(facts[0].temporal.is_none());
}

#[test]
fn ts6_partial_garbage_recovers_what_it_can() {
    // Mid-array garbage between two valid objects.
    let raw = r#"[
      {"subject":"A","predicate":"is","object":"x","salience":0.5,"evidence_turn_idx":1},
      this is not json,
      {"subject":"B","predicate":"is","object":"y","salience":0.4,"evidence_turn_idx":2}
    ]"#;
    let facts = parse_facts(raw, &turns_2()).unwrap();
    // Either both parsed (with line repair) or just one — either is acceptable
    // as long as we got at least one and didn't crash.
    assert!(facts.len() >= 1);
}

#[test]
fn ts6_hopeless_garbage_returns_empty_ok() {
    let raw = "no json here at all, just text";
    let facts = parse_facts(raw, &turns_2()).unwrap();
    assert!(facts.is_empty());
}

#[test]
fn ts6_out_of_range_evidence_turn_is_dropped() {
    let raw = r#"[
      {"subject":"X","predicate":"is","object":"y","salience":0.5,"evidence_turn_idx":99}
    ]"#;
    let facts = parse_facts(raw, &turns_2()).unwrap();
    // 99 is out of range for a 2-turn window — drop, don't crash.
    assert!(facts.is_empty());
}

#[test]
fn ts6_missing_salience_defaults_to_one() {
    let raw = r#"[
      {"subject":"X","predicate":"is","object":"y","evidence_turn_idx":1}
    ]"#;
    let facts = parse_facts(raw, &turns_2()).unwrap();
    assert_eq!(facts.len(), 1);
    assert!((facts[0].salience - 1.0).abs() < 1e-6);
}
