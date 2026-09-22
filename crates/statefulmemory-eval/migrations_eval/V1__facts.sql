-- Generated with AI Coding Rules Hub
-- V1__facts.sql — eval-side facts schema (lives in <data_dir>/<bench>/facts.db).
--
-- This is the eval crate's PRIVATE migration; the storage daemon's
-- migrations/V1__init.sql is unaffected (SC-10 tripwire). The schema
-- mirrors the storage layer's observations_fts trigger pattern so the
-- behaviour is familiar (compare migrations/V1__init.sql:59-74).
--
-- Spec: retrieval-upgrade-v1 §5. Plan: §1.1, P2 §4.2.

-- facts ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS facts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project         TEXT NOT NULL,
    evidence_obs_id INTEGER NOT NULL,
    subject         TEXT NOT NULL,
    predicate       TEXT NOT NULL,
    object          TEXT NOT NULL,
    temporal        TEXT,
    salience        REAL NOT NULL DEFAULT 1.0,
    source_session  TEXT,
    extracted_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_facts_project  ON facts(project);
CREATE INDEX IF NOT EXISTS idx_facts_evidence ON facts(evidence_obs_id);

-- facts_fts -----------------------------------------------------------------
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    subject, predicate, object, temporal,
    content='facts', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS facts_fts_ai AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, subject, predicate, object, temporal)
    VALUES (new.id, new.subject, new.predicate, new.object, new.temporal);
END;

CREATE TRIGGER IF NOT EXISTS facts_fts_ad AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, predicate, object, temporal)
    VALUES ('delete', old.id, old.subject, old.predicate, old.object, old.temporal);
END;

CREATE TRIGGER IF NOT EXISTS facts_fts_au AFTER UPDATE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, predicate, object, temporal)
    VALUES ('delete', old.id, old.subject, old.predicate, old.object, old.temporal);
    INSERT INTO facts_fts(rowid, subject, predicate, object, temporal)
    VALUES (new.id, new.subject, new.predicate, new.object, new.temporal);
END;

-- facts_vec -----------------------------------------------------------------
-- 384-dim BGE-small embeddings stored as f32 little-endian BLOBs.
-- Created here only if the sqlite-vec extension is loaded; FactsDb::open
-- enforces that order (load extension -> run migrations).
CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(embedding float[384]);

-- schema_meta ---------------------------------------------------------------
CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('version', '1');
