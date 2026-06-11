// Generated with AI Coding Rules Hub
//! TS-19: entity extractor produces a non-empty entity list for a
//! Caroline-and-Melanie sample window.

use std::sync::Arc;

use memlayer_extract::{
    claude_cli::MockClaudeClient,
    entities::{EntityExtractor, HaikuEntityExtractor, HeuristicEntityExtractor},
    Fact,
};

fn caroline_melanie_fact() -> Fact {
    Fact {
        subject: "Caroline".into(),
        predicate: "attended".into(),
        object: "LGBTQ support group with Melanie".into(),
        temporal: Some("2023-05-08".into()),
        salience: 0.9,
        evidence_obs_id: 101,
        source_session: Some("s-conv26".into()),
    }
}

#[tokio::test]
async fn ts19_heuristic_returns_non_empty_for_caroline_melanie() {
    let ex = HeuristicEntityExtractor::new();
    let ents = ex.extract(&caroline_melanie_fact()).await.unwrap();
    assert!(!ents.is_empty(), "expected at least one entity");
    // Should pick up the proper nouns.
    assert!(
        ents.contains(&"caroline".to_string()),
        "expected 'caroline' in {ents:?}"
    );
    assert!(
        ents.contains(&"melanie".to_string()),
        "expected 'melanie' in {ents:?}"
    );
}

#[tokio::test]
async fn ts19_haiku_returns_non_empty_for_caroline_melanie() {
    // Mock the LLM to return a structured response. The HaikuEntityExtractor
    // falls back to heuristic on parse failure, so even a janky mock would
    // yield entities — but here we exercise the happy-path JSON parse.
    let mock = Arc::new(MockClaudeClient::with_responses(vec![
        r#"{"entities": ["Caroline", "Melanie", "LGBTQ support group"]}"#.to_string(),
    ]));
    let ex = HaikuEntityExtractor::new(mock);
    let ents = ex.extract(&caroline_melanie_fact()).await.unwrap();
    assert!(ents.contains(&"caroline".to_string()), "got {ents:?}");
    assert!(ents.contains(&"melanie".to_string()), "got {ents:?}");
    assert!(
        ents.contains(&"lgbtq support group".to_string()),
        "got {ents:?}"
    );
}
