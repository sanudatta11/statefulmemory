// Generated with AI Coding Rules Hub
//! Skeleton TS-5: hybrid retrieval finds semantic synonym
//! ("paintings" → memory containing "art on canvas").
//!
//! Gated behind the `online-tests` feature because BGE-small must be
//! downloaded from HuggingFace Hub on first use. Default `cargo test`
//! pass MUST NOT pull network dependencies.
//!
//! Run with:
//!     cargo test -p memlayer-eval --features online-tests --test hybrid_retrieval

use std::path::PathBuf;
use std::sync::Arc;

use memlayer_embed::{cache::EmbeddingCache, BgeSmallEmbedder, Embedder};
use memlayer_eval::{
    datasets::EvalMemory,
    ingest::ingest_memories,
    retrieve_hybrid::retrieve_hybrid,
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

#[tokio::test]
#[cfg(feature = "online-tests")]
async fn ts5_paintings_finds_art_on_canvas() {
    let data_dir = fresh_data_dir("ts5-paintings");
    let session = uuid::Uuid::new_v4().to_string();
    let mems = vec![
        mk_mem(
            "ts5-proj",
            &session,
            "studio note",
            "Caroline made beautiful art on canvas this weekend.",
        ),
        mk_mem(
            "ts5-proj",
            &session,
            "weather",
            "Melanie said it was sunny outside.",
        ),
        mk_mem(
            "ts5-proj",
            &session,
            "lunch",
            "We had pasta for lunch yesterday.",
        ),
    ];
    ingest_memories(&data_dir, &mems, 1024)
        .await
        .expect("ingest");

    let embedder: Arc<dyn Embedder> =
        Arc::new(BgeSmallEmbedder::try_new().expect("load BGE-small"));
    let cache = Arc::new(EmbeddingCache::open(&data_dir).expect("open cache"));

    let result =
        retrieve_hybrid(&data_dir, "ts5-proj", "paintings", 3, embedder, cache)
            .await
            .expect("hybrid retrieve");

    assert!(!result.hits.is_empty(), "expected at least one hit");
    let top = &result.hits[0];
    assert!(
        top.contains("art on canvas") || top.contains("studio"),
        "expected art-on-canvas memory at top, got: {top}"
    );
}
