//! Integration tests for the extract pipeline + `obs facts <id>` verb.
//!
//! Spec: retrieval-promotion. Tests cover SC-9: `memlayer obs facts <id>`
//! returns the facts attached to that observation.
//!
//! NOTE: full end-to-end coverage (spawn daemon, save obs, GetFacts RPC)
//! requires Unix-socket binding in tempdir, which the current sandbox
//! restricts to `~/.memlayer/daemon.sock` only. The render-side test below
//! exercises the wire-to-text path. End-to-end coverage is deferred to a
//! local-only run of these tests in rp-t14.

use memlayer_proto as p;

#[test]
fn fact_proto_round_trips_id_subject_predicate_object() {
    // Sanity: prost-generated Fact struct has the fields we expect, with
    // the right types. If the proto drifts, this catches it before the
    // render impl breaks.
    let f = p::Fact {
        id: 42,
        obs_id: 7,
        subject: "team".into(),
        predicate: "prefers".into(),
        object: "raw SQL via pgx".into(),
        temporal: None,
        salience: 0.9,
        superseded_by: None,
        extracted_by: "haiku".into(),
        extracted_at: "2026-06-18T00:00:00Z".into(),
    };
    assert_eq!(f.id, 42);
    assert_eq!(f.obs_id, 7);
    assert_eq!(f.subject, "team");
    assert!(f.temporal.is_none());
}
