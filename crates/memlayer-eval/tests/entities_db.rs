// Generated with AI Coding Rules Hub
//! TS-20: entities + entity_links + entities_vec schema applies cleanly;
//! insert + lookup-by-name roundtrip works.

use memlayer_eval::entities_writer::{
    bulk_upsert_entities, link_entity_to_facts, upsert_entity, upsert_entity_vec, EntityRow,
};
use memlayer_eval::facts_db::FactsDb;
use rusqlite::params;
use tempfile::TempDir;

fn dummy_vec(seed: f32) -> Vec<f32> {
    (0..384).map(|i| seed + (i as f32 / 384.0)).collect()
}

fn insert_test_fact(
    conn: &rusqlite::Connection,
    project: &str,
    obs_id: i64,
    subject: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO facts(project, evidence_obs_id, subject, predicate, object, salience) \
         VALUES (?1, ?2, ?3, 'is', 'x', 1.0)",
        params![project, obs_id, subject],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn ts20_v2_migration_applies_cleanly() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("facts.db");
    let db = FactsDb::open(&path).unwrap();

    let v: String = db
        .conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v, "5");

    for name in &["entities", "entity_links"] {
        let count: i64 = db
            .conn
            .query_row(&format!("SELECT count(*) FROM {name}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{name} should be empty on fresh DB");
    }
    let n_vec: i64 = db
        .conn
        .query_row("SELECT count(*) FROM entities_vec", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n_vec, 0);

    drop(db);
    let _db2 = FactsDb::open(&path).unwrap();
}

#[test]
fn ts20_insert_and_lookup_entity_by_name() {
    let tmp = TempDir::new().unwrap();
    let mut db = FactsDb::open(&tmp.path().join("facts.db")).unwrap();
    let fact_id = insert_test_fact(&db.conn, "test-proj", 100, "Caroline");

    let id = upsert_entity(&db.conn, "test-proj", "caroline", Some("person")).unwrap();
    link_entity_to_facts(&db.conn, id, &[fact_id]).unwrap();
    upsert_entity_vec(&db.conn, id, &dummy_vec(0.1)).unwrap();

    let lookup: i64 = db
        .conn
        .query_row(
            "SELECT id FROM entities WHERE project='test-proj' AND name='caroline'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lookup, id);

    let n_links: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM entity_links WHERE entity_id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_links, 1);

    let n_vec: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM entities_vec WHERE rowid = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_vec, 1);

    let _ = bulk_upsert_entities(
        &mut db.conn,
        "test-proj",
        &[EntityRow {
            name: "caroline".into(),
            embedding: dummy_vec(0.2),
            linked_fact_ids: vec![fact_id],
            kind: None,
        }],
    )
    .unwrap();
    let total: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM entities WHERE project='test-proj'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total, 1);
}
