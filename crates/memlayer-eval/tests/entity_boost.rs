// Generated with AI Coding Rules Hub
//! TS-21: entity boost wiring through retrieve_facts.
//!
//! End-to-end test that a query mentioning a known entity name boosts the
//! linked fact above one that shares only weak BM25 / vector signal.
//!
//! Strategy: drive the storage DB through the public `ingest_memories` API
//! (the same path the eval runner uses) so we don't have to reimplement
//! migrations or worry about FKs in test code. Then we open the resulting
//! facts.db (which `extract_pipeline` populates), but for this isolated
//! TS-21 unit we instead build facts.db directly to keep the test offline
//! and avoid pulling in the LLM extractor.
//!
//! To keep this offline-default we use a deterministic stub embedder that
//! returns the same 384-dim vector for every input. Vector similarity
//! therefore gives every fact identical sem signal — the test isolates
//! the **entity-name match path** (project + name lookup) from the
//! entity-vec ANN path. Both paths drive the same boost in production;
//! pinning down the SQL name-match path here ensures CI never depends on
//! the BGE download.
//!
//! Spec: retrieval-upgrade-v1 §6, TS-21, SC-13. Plan: P2 §4.5.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rusqlite::params;

use memlayer_embed::{cache::EmbeddingCache, Embedder};
use memlayer_eval::datasets::EvalMemory;
use memlayer_eval::entities_writer::{bulk_upsert_entities, EntityRow};
use memlayer_eval::facts_db::FactsDb;
use memlayer_eval::ingest::ingest_memories;
use memlayer_eval::retrieve_facts::retrieve_facts;
use memlayer_storage::ProjectRegistry;

/// Stub embedder that returns the same constant 384-dim vector for every
/// input. Makes vector-similarity ranking degenerate so the test isolates
/// the entity-name SQL match path.
struct ConstantEmbedder;

impl Embedder for ConstantEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.5_f32; 384]).collect())
    }
}

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

/// Insert a fact and a vec0 row pointing at the given evidence_obs_id.
fn insert_fact(
    facts_db: &FactsDb,
    project: &str,
    obs_id: i64,
    subject: &str,
    predicate: &str,
    object: &str,
) -> i64 {
    facts_db
        .conn
        .execute(
            "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, salience) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project, obs_id, subject, predicate, object, 1.0_f64],
        )
        .expect("insert fact");
    facts_db.conn.last_insert_rowid()
}

fn insert_fact_vec(facts_db: &FactsDb, fact_id: i64, vec: &[f32]) {
    let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    facts_db
        .conn
        .execute(
            "INSERT INTO facts_vec(rowid, embedding) VALUES (?1, ?2)",
            params![fact_id, bytes],
        )
        .expect("insert facts_vec row");
}

#[tokio::test]
async fn ts21_entity_name_match_boosts_linked_fact_above_unlinked() {
    let data_dir = fresh_data_dir("ts21-entity-boost");
    std::env::set_var("MEMLAYER_DATA_DIR", &data_dir);

    let project = "ts21-proj";
    let session_id = uuid::Uuid::new_v4().to_string();

    // Drive the storage DB up via the same code path the runner uses. This
    // installs the schema and populates 3 observations whose ids we can
    // reference from facts.evidence_obs_id.
    let mems = vec![
        mk_mem(project, &session_id, "Caroline trip", "Caroline went to Paris last summer"),
        mk_mem(project, &session_id, "Melanie hobby", "Melanie enjoys painting on weekends"),
        mk_mem(project, &session_id, "Generic note", "Painting is a relaxing hobby"),
    ];
    ingest_memories(&data_dir, &mems, 1024)
        .await
        .expect("ingest test memories");

    // Look up the resulting observation ids so facts.evidence_obs_id is valid.
    let registry = Arc::new(ProjectRegistry::new(
        64,
        1,
        std::time::Duration::from_millis(50),
    ));
    let project_state = registry.get_or_open(project).expect("open storage project");
    let storage_conn = project_state.open_read_conn().expect("open storage read conn");
    let obs_ids: Vec<i64> = {
        let mut stmt = storage_conn
            .prepare("SELECT id FROM observations WHERE session_id=?1 ORDER BY id ASC")
            .unwrap();
        let rows = stmt
            .query_map(params![session_id], |r| r.get::<_, i64>(0))
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    };
    assert_eq!(obs_ids.len(), 3, "expected 3 observations from ingest");

    // Build facts.db directly (no LLM extraction).
    let facts_db_path = data_dir.join("facts.db");
    let mut facts_db = FactsDb::open(&facts_db_path).expect("open facts.db");

    let f1_id = insert_fact(&facts_db, project, obs_ids[0], "Caroline", "went-to", "Paris");
    let f2_id = insert_fact(&facts_db, project, obs_ids[1], "Melanie", "enjoys", "painting");
    let f3_id = insert_fact(&facts_db, project, obs_ids[2], "Painting", "is-a", "hobby");

    // Identical embeddings across all three -> ANN distances all equal,
    // sem signal degenerate. BM25 can still distinguish them.
    let v = vec![0.5_f32; 384];
    insert_fact_vec(&facts_db, f1_id, &v);
    insert_fact_vec(&facts_db, f2_id, &v);
    insert_fact_vec(&facts_db, f3_id, &v);

    // Wire entities: "caroline" -> fact 1, "melanie" -> fact 2.
    bulk_upsert_entities(
        &mut facts_db.conn,
        project,
        &[
            EntityRow {
                name: "caroline".into(),
                embedding: vec![0.5_f32; 384],
                linked_fact_ids: vec![f1_id],
                kind: Some("person".into()),
            },
            EntityRow {
                name: "melanie".into(),
                embedding: vec![0.5_f32; 384],
                linked_fact_ids: vec![f2_id],
                kind: Some("person".into()),
            },
        ],
    )
    .expect("bulk upsert entities");

    drop(facts_db);

    // Run retrieval. The query mentions "Caroline" so the heuristic
    // tokenizer will yield {"caroline", ...} which matches the entity row
    // linked to f1.
    let cache = Arc::new(EmbeddingCache::open(&data_dir).expect("open embedding cache"));
    let embedder: Arc<dyn Embedder> = Arc::new(ConstantEmbedder);

    let result = retrieve_facts(
        &data_dir,
        &facts_db_path,
        project,
        "Where did Caroline travel last summer?",
        3,
        0,
        embedder,
        cache,
        0.005,
    )
    .await
    .expect("retrieve_facts must succeed");

    assert!(
        !result.hits.is_empty(),
        "must return at least one hit (got 0)",
    );

    // Fact 1 (Caroline -> Paris) must rank above fact 3 (Painting -> hobby).
    let f1_pos = result
        .hits
        .iter()
        .position(|h| h.contains("Caroline") && h.contains("Paris"));
    let f3_pos = result
        .hits
        .iter()
        .position(|h| h.contains("Painting") && h.contains("hobby"));

    let f1_pos = f1_pos.expect("fact 1 must appear in results");
    if let Some(p3) = f3_pos {
        assert!(
            f1_pos < p3,
            "fact 1 (entity-linked) must rank above fact 3 (unlinked); got positions {f1_pos} vs {p3}; hits={:?}",
            result.hits
        );
    }
    // Top hit should be fact 1.
    assert_eq!(
        f1_pos, 0,
        "fact 1 must be the top hit; got hits={:?}",
        result.hits
    );
}
