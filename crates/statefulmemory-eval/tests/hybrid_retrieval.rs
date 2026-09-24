// Generated with AI Coding Rules Hub
//! Skeleton TS-5: hybrid retrieval finds semantic synonym
//! ("paintings" → memory containing "art on canvas").
//!
//! Gated behind the `online-tests` feature because BGE-small must be
//! downloaded from HuggingFace Hub on first use. Default `cargo test`
//! pass MUST NOT pull network dependencies.
//!
//! Run with:
//!     cargo test -p statefulmemory-eval --features online-tests --test hybrid_retrieval

#![cfg_attr(not(feature = "online-tests"), allow(dead_code, unused_imports))]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use statefulmemory_embed::{cache::EmbeddingCache, BgeSmallEmbedder, Embedder};
use statefulmemory_eval::{
    datasets::EvalMemory,
    ingest::ingest_memories,
    retrieve_hybrid::{retrieve_hybrid, retrieve_hybrid_production_profile},
};
use statefulmemory_retrieval::hybrid;
use statefulmemory_storage::{
    facts as facts_q, read as read_q,
    write::{NewFact, WriteRequest},
    ProjectRegistry,
};
use tokio::sync::oneshot;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct FixedEmbedder;

impl Embedder for FixedEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0; 384];
                if *text == "lexical" {
                    vector[1] = 1.0;
                } else if text.contains("lexical") {
                    vector[0] = 1.0;
                } else if text.contains("dense") {
                    vector[2] = 1.0;
                } else {
                    vector[3] = 1.0;
                }
                vector
            })
            .collect())
    }

    fn dim(&self) -> usize {
        384
    }
}

fn fresh_data_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("statefulmemory-eval-tests")
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
async fn production_profile_candidates_match_storage_lanes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let data_dir = fresh_data_dir("production-profile");
    let session = uuid::Uuid::new_v4().to_string();
    let memories = vec![
        mk_mem(
            "production-profile",
            &session,
            "lexical note",
            "The lexical marker appears in this observation.",
        ),
        mk_mem(
            "production-profile",
            &session,
            "dense note",
            "A semantically related observation without the query marker.",
        ),
        mk_mem(
            "production-profile",
            &session,
            "unrelated",
            "Nothing relevant appears here.",
        ),
    ];
    ingest_memories(&data_dir, &memories, 1024, false)
        .await
        .expect("ingest");

    let registry = Arc::new(ProjectRegistry::new(64, 1, Duration::from_millis(50)));
    let project_state = registry
        .get_or_open("production-profile")
        .expect("open project");
    let conn = project_state.open_read_conn().expect("open read conn");
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, content FROM observations \
                 WHERE deleted_at IS NULL ORDER BY id",
            )
            .expect("prepare observations");
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("query observations")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect observations")
    };
    drop(conn);

    let lexical_id: i64 = rows.first().map(|row| row.0).expect("fixture row");
    let embedder = Arc::new(FixedEmbedder);
    let cache = Arc::new(EmbeddingCache::open(&data_dir).expect("open cache"));
    statefulmemory_eval::retrieve_hybrid::prewarm_project_embeddings(
        &data_dir,
        &["production-profile".into()],
        embedder.clone(),
        cache.clone(),
    )
    .await
    .expect("prewarm project embeddings");
    let (reply, receiver) = oneshot::channel();
    project_state
        .write
        .send(WriteRequest::InsertFacts {
            obs_id: lexical_id,
            facts: vec![NewFact {
                subject: "lexical".into(),
                predicate: "mentions".into(),
                object: "marker".into(),
                temporal: None,
                salience: Some(0.9),
                extracted_by: "test".into(),
            }],
            reply,
        })
        .expect("send fact");
    receiver.await.expect("fact reply").expect("store fact");

    let result = retrieve_hybrid_production_profile(
        &data_dir,
        "production-profile",
        "lexical",
        10,
        embedder.clone(),
        cache,
    )
    .await
    .expect("production profile retrieval");

    let query_vec = embedder.embed(&["lexical"]).expect("embed query").remove(0);
    let conn = project_state
        .open_read_conn()
        .expect("open verification conn");
    let bm25 = read_q::search(
        &conn,
        "lexical",
        None,
        None,
        hybrid::DEFAULT_CANDIDATE_DEPTH,
    )
    .expect("bm25 lane");
    let dense = read_q::search_dense(
        &conn,
        &query_vec,
        i64::from(hybrid::DEFAULT_CANDIDATE_DEPTH),
    )
    .expect("dense lane");
    let fact_parent_ids = facts_q::search_observation_ids(
        &conn,
        "lexical",
        i64::from(hybrid::DEFAULT_CANDIDATE_DEPTH),
    )
    .expect("fact lane");
    let expected = hybrid::fuse_candidate_trace(
        &bm25.iter().map(|o| o.id as u64).collect::<Vec<_>>(),
        &dense.iter().map(|o| o.id as u64).collect::<Vec<_>>(),
        &fact_parent_ids,
        hybrid::DEFAULT_RRF_K,
    );

    assert_eq!(result.candidates, expected);
    assert!(result.candidates.ids().contains(&(lexical_id as u64)));
    assert_eq!(result.candidates.fact_parent_ids, vec![lexical_id as u64]);
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
    ingest_memories(&data_dir, &mems, 1024, false)
        .await
        .expect("ingest");

    let embedder: Arc<dyn Embedder> =
        Arc::new(BgeSmallEmbedder::try_new().expect("load BGE-small"));
    let cache = Arc::new(EmbeddingCache::open(&data_dir).expect("open cache"));

    let result = retrieve_hybrid(&data_dir, "ts5-proj", "paintings", 3, embedder, cache)
        .await
        .expect("hybrid retrieve");

    assert!(!result.hits.is_empty(), "expected at least one hit");
    let top = &result.hits[0];
    assert!(
        top.contains("art on canvas") || top.contains("studio"),
        "expected art-on-canvas memory at top, got: {top}"
    );
}
