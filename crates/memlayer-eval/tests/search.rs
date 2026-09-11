// Generated with AI Coding Rules Hub
//! Integration tests for memory search end-to-end.
//!
//! Inserts a tiny, known fixture into a fresh per-test data dir, then runs
//! `retrieve` against it and asserts on the returned hits. Verifies that the
//! tokenizer + FTS5 + storage layer cooperate for realistic query shapes
//! (multi-word, stopwords, punctuation, etc.) — the failure modes that 0%
//! benchmark accuracy surfaced.

use std::path::PathBuf;
use std::sync::Mutex;

use memlayer_eval::{
    datasets::EvalMemory,
    ingest::ingest_memories,
    retrieve::retrieve,
};

/// `MEMLAYER_DATA_DIR` is process-global; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    ingest_memories(data_dir, memories, 1024)
        .await
        .expect("ingest should succeed");
}

#[tokio::test]
async fn single_keyword_query_finds_matching_memory() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("single-keyword");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        mk_mem("test-proj", &session, "Caroline May 8",
               "Caroline: I went to the LGBTQ support group on May 8th."),
        mk_mem("test-proj", &session, "Melanie weather",
               "Melanie: It was sunny today, I went swimming."),
        mk_mem("test-proj", &session, "Caroline studies",
               "Caroline: I'm thinking about psychology and counseling."),
    ];
    ingest(&data_dir, &memories).await;

    let result = retrieve(&data_dir, "test-proj", "Caroline", 10)
        .expect("retrieve should succeed");
    assert!(!result.hits.is_empty(), "expected hits for 'Caroline'");
    assert!(
        result.hits.iter().all(|h| h.contains("Caroline")),
        "all hits should contain Caroline; got {:?}",
        result.hits
    );
    assert!(result.hits.len() >= 2, "expected at least 2 Caroline hits");
}

#[tokio::test]
async fn natural_language_question_with_punctuation_returns_hits() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("nl-question");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        mk_mem("nl-proj", &session, "support group attendance",
               "Caroline: I attended the LGBTQ support group on May 8, 2023."),
        mk_mem("nl-proj", &session, "small talk",
               "Melanie: Hey Caroline, how's it going today?"),
        mk_mem("nl-proj", &session, "painting",
               "Melanie: I painted a sunrise last weekend."),
    ];
    ingest(&data_dir, &memories).await;

    let result = retrieve(
        &data_dir,
        "nl-proj",
        "When did Caroline go to the LGBTQ support group?",
        10,
    )
    .expect("retrieve should succeed");

    assert!(!result.hits.is_empty(),
        "natural-language question should match memories despite punctuation");
    let top = &result.hits[0];
    assert!(
        top.contains("LGBTQ") || top.contains("support group"),
        "top hit should be the support-group memory; got: {top}"
    );
}

#[tokio::test]
async fn stopword_only_query_does_not_panic() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("stopword");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        mk_mem("sw-proj", &session, "fact one", "Caroline likes psychology."),
    ];
    ingest(&data_dir, &memories).await;

    // All-stopword query — must not panic; empty hits are acceptable.
    let result = retrieve(&data_dir, "sw-proj", "what is the?", 10)
        .expect("retrieve should not error on stopword-only input");
    assert!(result.hits.is_empty() || !result.hits.is_empty(),
        "either is fine; the contract is `does not panic / does not error`");
}

#[tokio::test]
async fn project_isolation_no_cross_project_leakage() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("isolation");
    let session = uuid::Uuid::new_v4().to_string();
    let mems_a = vec![
        mk_mem("proj-a", &session, "alpha note",
               "Caroline visited the museum in Paris."),
    ];
    let mems_b = vec![
        mk_mem("proj-b", &session, "beta note",
               "Melanie went hiking in the Alps."),
    ];
    ingest(&data_dir, &mems_a).await;
    ingest(&data_dir, &mems_b).await;

    // Query proj-a for Caroline → should match.
    let r_a = retrieve(&data_dir, "proj-a", "Caroline museum Paris", 10).unwrap();
    assert!(!r_a.hits.is_empty(), "proj-a should return Caroline memory");
    assert!(r_a.hits.iter().any(|h| h.contains("Paris")));

    // Query proj-a for Melanie → should NOT match (it's in proj-b).
    let r_a_other = retrieve(&data_dir, "proj-a", "Melanie hiking Alps", 10).unwrap();
    assert!(
        r_a_other.hits.iter().all(|h| !h.contains("Alps")),
        "proj-a must not leak proj-b memories; got {:?}",
        r_a_other.hits
    );

    // Query proj-b for Melanie → should match.
    let r_b = retrieve(&data_dir, "proj-b", "Melanie hiking Alps", 10).unwrap();
    assert!(!r_b.hits.is_empty(), "proj-b should return Melanie memory");
}

#[tokio::test]
async fn k_limits_returned_results() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("k-limit");
    let session = uuid::Uuid::new_v4().to_string();
    let memories: Vec<EvalMemory> = (0..15)
        .map(|i| {
            mk_mem(
                "k-proj",
                &session,
                &format!("note {i}"),
                &format!("Caroline mentioned topic number {i} briefly."),
            )
        })
        .collect();
    ingest(&data_dir, &memories).await;

    let result = retrieve(&data_dir, "k-proj", "Caroline topic", 5).unwrap();
    assert!(result.hits.len() <= 5, "k=5 must cap returned hits; got {}", result.hits.len());
    assert!(result.hits.len() >= 1, "should still return some hits");
}

#[tokio::test]
async fn ranking_prefers_more_specific_match() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = fresh_data_dir("ranking");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        mk_mem("rank-proj", &session, "many words",
               "Caroline mentioned weather briefly in passing."),
        mk_mem("rank-proj", &session, "specific match",
               "Caroline visited the LGBTQ support group on Tuesday."),
        mk_mem("rank-proj", &session, "unrelated",
               "Melanie cooked dinner and watched a movie."),
    ];
    ingest(&data_dir, &memories).await;

    let result = retrieve(&data_dir, "rank-proj",
        "Caroline LGBTQ support group", 10).unwrap();
    assert!(!result.hits.is_empty());
    let top = &result.hits[0];
    assert!(
        top.contains("LGBTQ") && top.contains("support group"),
        "top hit should be the most specific match; got: {top}"
    );
}
