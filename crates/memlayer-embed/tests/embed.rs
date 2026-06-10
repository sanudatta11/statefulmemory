// Generated with AI Coding Rules Hub
//! Integration tests for `memlayer-embed`.
//!
//! TS-3: cache hit returns identical bytes.
//! EH-7: fingerprint collision treated as miss.
//! TS-2: BGE-small embeddings are deterministic on CPU.
//!
//! Tests use `tempfile::TempDir` so they're hermetic — no shared global state,
//! no env-var contention.

use memlayer_embed::cache::{fingerprint, hash_key, EmbeddingCache};
use tempfile::TempDir;

/// Sample 4-dim vectors, easy to compare exactly.
fn sample_vecs() -> Vec<(String, Vec<f32>)> {
    vec![
        ("hello world".to_string(), vec![0.1_f32, 0.2, 0.3, 0.4]),
        ("rust is fun".to_string(), vec![-0.5_f32, 0.0, 1.0, std::f32::consts::PI]),
        ("memlayer".to_string(), vec![1e-6_f32, -1e-6, 42.0, -42.0]),
    ]
}

/// TS-3: put then get returns the exact same f32 bytes.
#[test]
fn cache_put_then_get_returns_identical_bytes() {
    let tmp = TempDir::new().expect("tempdir");
    let cache = EmbeddingCache::open(tmp.path()).expect("open cache");

    let items = sample_vecs();
    cache.put_many(&items).expect("put_many");

    let texts: Vec<&str> = items.iter().map(|(t, _)| t.as_str()).collect();
    let (hits, miss_indices) = cache.get_many(&texts).expect("get_many");

    assert!(miss_indices.is_empty(), "all entries should hit");
    assert_eq!(hits.len(), items.len());

    for (i, (_, expected)) in items.iter().enumerate() {
        let got = hits[i].as_ref().expect("hit");
        assert_eq!(got.len(), expected.len(), "vector len mismatch at {i}");
        // Exact byte equality via to_le_bytes.
        for (j, (a, b)) in got.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                a.to_le_bytes(),
                b.to_le_bytes(),
                "byte mismatch at vec[{i}][{j}]"
            );
        }
    }
}

/// Misses return None and miss_indices in the right order.
#[test]
fn cache_miss_returns_none_with_correct_indices() {
    let tmp = TempDir::new().expect("tempdir");
    let cache = EmbeddingCache::open(tmp.path()).expect("open cache");

    cache
        .put_many(&[("a".to_string(), vec![1.0_f32, 2.0])])
        .expect("put_many");

    let (hits, miss_indices) = cache.get_many(&["a", "b", "c"]).expect("get_many");

    assert_eq!(hits.len(), 3);
    assert!(hits[0].is_some(), "a should hit");
    assert!(hits[1].is_none(), "b should miss");
    assert!(hits[2].is_none(), "c should miss");
    assert_eq!(miss_indices, vec![1, 2]);
}

/// EH-7: a row keyed by hash(a) but with a fingerprint from a different text
/// must be treated as a miss when querying for "a".
#[test]
fn cache_fingerprint_collision_treated_as_miss() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("embeddings.cache.sqlite");

    {
        let cache = EmbeddingCache::open(tmp.path()).expect("open cache");
        // Force the schema to exist by issuing a no-op put_many.
        cache.put_many(&[]).expect("put_many empty");
    }

    // Manually craft a row with key = sha256("a") but fingerprint from
    // an unrelated text. Querying for "a" must detect the mismatch and miss.
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open raw");
        let key = hash_key("a");
        let bogus_fingerprint = fingerprint("totally different source text");
        let payload: Vec<u8> = [9.9_f32, 8.8].iter().flat_map(|f| f.to_le_bytes()).collect();
        conn.execute(
            "INSERT OR REPLACE INTO cache (key, fingerprint, payload) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, bogus_fingerprint, payload],
        )
        .expect("manual insert");
    }

    let cache = EmbeddingCache::open(tmp.path()).expect("reopen cache");
    let (hits, miss_indices) = cache.get_many(&["a"]).expect("get_many");

    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].is_none(),
        "fingerprint mismatch must be treated as miss (EH-7)"
    );
    assert_eq!(miss_indices, vec![0]);
}

// ---------------------------------------------------------------------------
// TS-2 — BGE-small embedder determinism + smoke test.
//
// These are gated behind the `online-tests` feature because the first run
// downloads BAAI/bge-small-en-v1.5 from HuggingFace Hub (~130 MB). The default
// `cargo test` invocation must NOT trigger network I/O.
//
// To run them explicitly:
//   cargo test --release -p memlayer-embed --features online-tests \
//     -- --test-threads=1 --include-ignored
// ---------------------------------------------------------------------------

/// TS-2: identical input must produce byte-equal output across calls (CPU,
/// no dropout in inference, deterministic kernels).
#[test]
#[cfg_attr(not(feature = "online-tests"), ignore)]
fn bge_small_embedding_is_deterministic_for_same_text() {
    let emb = memlayer_embed::embedder::BgeSmallEmbedder::try_new()
        .expect("load BGE-small (requires HF Hub network access on first run)");
    let v1 = emb.embed(&["hello world"]).unwrap().remove(0);
    let v2 = emb.embed(&["hello world"]).unwrap().remove(0);
    assert_eq!(v1.len(), 384);
    assert_eq!(v1.len(), memlayer_embed::Embedder::dim(&emb));
    // Byte-exact equality on CPU with same model state.
    let bytes1: Vec<u8> = v1.iter().flat_map(|f| f.to_le_bytes()).collect();
    let bytes2: Vec<u8> = v2.iter().flat_map(|f| f.to_le_bytes()).collect();
    assert_eq!(bytes1, bytes2, "embeddings must be byte-equal across calls");
}

/// Smoke: distinct sentences should not collapse onto each other after
/// L2-normalization. Cosine similarity (= dot product on normalized vectors)
/// stays comfortably below 1.0 for unrelated text.
#[test]
#[cfg_attr(not(feature = "online-tests"), ignore)]
fn bge_small_distinct_texts_have_distinct_embeddings() {
    let emb = memlayer_embed::embedder::BgeSmallEmbedder::try_new().unwrap();
    let v1 = emb.embed(&["the cat sat on the mat"]).unwrap().remove(0);
    let v2 = emb.embed(&["airplanes fly through the sky"]).unwrap().remove(0);
    let cosine: f32 = v1.iter().zip(v2.iter()).map(|(a, b)| a * b).sum();
    // After L2-normalization, dot product is cosine similarity.
    // Different sentences should be < 0.9.
    assert!(cosine < 0.9, "expected distinct embeddings, got cosine {cosine}");
}
