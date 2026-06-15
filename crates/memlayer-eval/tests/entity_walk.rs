// Generated with AI Coding Rules Hub
//! TS-19: 2-hop entity walk surfaces a fact connected through a shared
//! co-entity even when the fact has no direct BM25 / dense / single-hop
//! entity-boost hit.
//!
//! Graph layout used by the test:
//!
//!   entity Caroline ── fact F1 ── entity Sweden
//!                                     │
//!                                     ├─ fact F2 (target, second-order)
//!                                     │
//!   entity Mel      ── fact F3 ── entity Sweden
//!
//! Query entities: ["caroline"]. Hop 1 surfaces F1 directly (already
//! credited by entity_boost). Hop 2 walks Caroline → F1 → Sweden →
//! [F2, F3]. F2 should appear in the walk_boost map; the query's
//! direct-link facts (F1) must NOT appear (we exclude hop-1 from
//! walk credit to avoid double-counting).
//!
//! Spec link: TS-19. Plan: analysis.md §6.5.

use rusqlite::params;
use tempfile::TempDir;

use memlayer_eval::entity_walk::compute_entity_walk_boost;
use memlayer_eval::facts_db::FactsDb;

fn seed_graph(db_path: &std::path::Path, project: &str) {
    let _ = FactsDb::open(db_path).expect("open facts.db");
    let conn = rusqlite::Connection::open(db_path).expect("plain conn");
    // Three facts.
    for (subj, pred, obj) in [
        ("caroline", "moved_from", "sweden"),  // F1
        ("sweden", "has_climate", "cold"),     // F2 — second-order target
        ("mel", "visited", "sweden"),          // F3 — second-order via Sweden
    ] {
        conn.execute(
            "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, salience) \
                  VALUES (?1, 0, ?2, ?3, ?4, 1.0)",
            params![project, subj, pred, obj],
        )
        .unwrap();
    }
    let f1: i64 = conn.last_insert_rowid() - 2;
    let f2: i64 = f1 + 1;
    let f3: i64 = f2 + 1;

    // Three entities.
    for name in ["caroline", "sweden", "mel"] {
        conn.execute(
            "INSERT INTO entities(project, name, kind) VALUES (?1, ?2, 'person')",
            params![project, name],
        )
        .unwrap();
    }
    let caroline_id: i64 = conn
        .query_row(
            "SELECT id FROM entities WHERE project=?1 AND name='caroline'",
            params![project],
            |r| r.get(0),
        )
        .unwrap();
    let sweden_id: i64 = conn
        .query_row(
            "SELECT id FROM entities WHERE project=?1 AND name='sweden'",
            params![project],
            |r| r.get(0),
        )
        .unwrap();
    let mel_id: i64 = conn
        .query_row(
            "SELECT id FROM entities WHERE project=?1 AND name='mel'",
            params![project],
            |r| r.get(0),
        )
        .unwrap();

    // Links: F1↔Caroline, F1↔Sweden, F2↔Sweden, F3↔Mel, F3↔Sweden.
    for (eid, fid) in [
        (caroline_id, f1),
        (sweden_id, f1),
        (sweden_id, f2),
        (sweden_id, f3),
        (mel_id, f3),
    ] {
        conn.execute(
            "INSERT INTO entity_links(entity_id, fact_id) VALUES (?1, ?2)",
            params![eid, fid],
        )
        .unwrap();
    }
}

#[test]
fn ts19_walk_surfaces_second_order_fact_via_shared_co_entity() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("facts.db");
    let project = "p";
    seed_graph(&db, project);

    let qe = vec!["caroline".to_string()];
    let boosts = compute_entity_walk_boost(&db, project, &qe).unwrap();

    // F1 is hop-1 (Caroline's direct link) — must be excluded from walk
    // credit (the existing entity_boost gives it the direct lift).
    let conn = rusqlite::Connection::open(&db).unwrap();
    let f1: i64 = conn
        .query_row(
            "SELECT id FROM facts WHERE subject='caroline' AND predicate='moved_from'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let f2: i64 = conn
        .query_row(
            "SELECT id FROM facts WHERE subject='sweden' AND predicate='has_climate'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let f3: i64 = conn
        .query_row(
            "SELECT id FROM facts WHERE subject='mel' AND predicate='visited'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    assert!(!boosts.contains_key(&f1), "hop-1 fact must be excluded");
    assert!(
        boosts.contains_key(&f2),
        "F2 (Sweden's other fact) must surface; got {boosts:?}"
    );
    assert!(
        boosts.contains_key(&f3),
        "F3 (Mel's fact via Sweden bridge) must also surface"
    );
    // Path increment is 0.1 — single-path facts get exactly 0.1.
    assert!((boosts[&f2] - 0.1).abs() < 1e-6);
}

#[test]
fn ts19_empty_query_entities_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("facts.db");
    seed_graph(&db, "p");
    let boosts = compute_entity_walk_boost(&db, "p", &[]).unwrap();
    assert!(boosts.is_empty());
}

#[test]
fn ts19_unknown_query_entity_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("facts.db");
    seed_graph(&db, "p");
    let qe = vec!["nonexistent".to_string()];
    let boosts = compute_entity_walk_boost(&db, "p", &qe).unwrap();
    assert!(boosts.is_empty());
}
