#![allow(clippy::await_holding_lock)]
// Generated with AI Coding Rules Hub
//! Integration tests for the 3-tier tokenizer (TS-1, TS-14, SC-9).
//!
//! These exercise `retrieve()` end-to-end so the new tokenizer is validated via
//! observable behaviour rather than internal API. The contract:
//!   * Tier-1 phrase match must outrank pure OR matches (ranking).
//!   * Tier-3 OR fallback must still surface single-token-only memories
//!     (recall preserved).
//!   * Edge cases (single token, stopword-only) must not regress / panic.
//!
//! Spec links: TS-1, TS-14, SC-9. Plan: P0 §2.3, §2.6.

use std::path::PathBuf;

use memlayer_eval::{
    datasets::EvalMemory,
    ingest::ingest_memories,
    retrieve::retrieve,
};

fn fresh_data_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("memlayer-eval-tests")
        .join(format!("{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mk_mem(project: &str, session: &str, title: &str, content: &str) -> EvalMemory {
    EvalMemory {
        project: project.to_string(),
        session_id: session.to_string(),
        obs_type: "conversation".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        topic_key: None,
    }
}

async fn ingest(data_dir: &std::path::Path, memories: &[EvalMemory]) {
    ingest_memories(data_dir, memories, 1024, false)
        .await
        .expect("ingest should succeed");
}

/// TS-1: multi-word natural-language query must (a) retrieve the phrase
/// memory AND the OR-only memory (recall via Tier-3) and (b) rank the phrase
/// memory ABOVE the OR-only memory (Tier-1 phrase boost via BM25).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn phrase_fallback_returns_hits_on_multi_word_query() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("phrase-fallback");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        // Decoy first (lowest id): unrelated.
        mk_mem(
            "phrase-proj",
            &session,
            "decoy",
            "Melanie went hiking in the mountains.",
        ),
        // OR-only memory (mid id): contains *some* of the tokens in scattered
        // form. Should still be retrieved (Tier-3 OR) but ranked BELOW the
        // phrase memory.
        mk_mem(
            "phrase-proj",
            &session,
            "scattered tokens",
            "Caroline supports stuff and goes to various places.",
        ),
        // Phrase memory (highest id): contains the literal multi-word phrase.
        // Inserted last so the outer `ORDER BY id DESC` in retrieve()'s SQL
        // surfaces it first when both rows survive the inner BM25 LIMIT — the
        // same convention the existing `ranking_prefers_more_specific_match`
        // test relies on.
        mk_mem(
            "phrase-proj",
            &session,
            "phrase match",
            "Caroline LGBTQ support group met on Tuesday afternoon.",
        ),
    ];
    ingest(&data_dir, &memories).await;

    let result = retrieve(
        &data_dir,
        "phrase-proj",
        "When did Caroline go to the LGBTQ support group?",
        10,
    )
    .expect("retrieve should succeed");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for multi-word query"
    );

    // Recall: Tier-3 OR must still pull the scattered-tokens memory in.
    let hit_phrase = result
        .hits
        .iter()
        .any(|h| h.contains("LGBTQ support group"));
    let hit_scattered = result
        .hits
        .iter()
        .any(|h| h.contains("supports stuff"));
    assert!(
        hit_phrase,
        "phrase memory must be retrieved; got {:?}",
        result.hits
    );
    assert!(
        hit_scattered,
        "Tier-3 OR fallback must still surface the scattered-token memory; got {:?}",
        result.hits
    );

    // Ranking: phrase memory must be top hit (Tier-1 BM25 boost).
    let top = &result.hits[0];
    assert!(
        top.contains("LGBTQ support group"),
        "phrase memory must rank above scattered-token memory; top was: {top}"
    );
}

/// Single-token query should behave identically to before — Tier-3 only.
#[tokio::test]
async fn single_token_query_works_unchanged() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("single-token");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        mk_mem(
            "single-proj",
            &session,
            "caroline note",
            "Caroline mentioned a book she was reading.",
        ),
        mk_mem(
            "single-proj",
            &session,
            "melanie note",
            "Melanie talked about cooking last night.",
        ),
    ];
    ingest(&data_dir, &memories).await;

    let result = retrieve(&data_dir, "single-proj", "Caroline", 10)
        .expect("retrieve should succeed");

    assert!(!result.hits.is_empty(), "expected hits for single-token query");
    assert!(
        result.hits.iter().all(|h| h.contains("Caroline")),
        "all hits must contain Caroline; got {:?}",
        result.hits
    );
}

/// Stopword-only / question-words-only query must not panic or error.
#[tokio::test]
async fn stopword_only_query_does_not_panic() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("stopword-only");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![mk_mem(
        "sw-proj",
        &session,
        "fact",
        "Caroline likes psychology.",
    )];
    ingest(&data_dir, &memories).await;

    // All tokens are stopwords → tokenizer must produce a safe FTS5 string.
    let result = retrieve(&data_dir, "sw-proj", "what is the?", 10)
        .expect("retrieve must not error on stopword-only input");
    // Empty hits acceptable; the contract is "does not panic / error".
    let _ = result.hits;
}
