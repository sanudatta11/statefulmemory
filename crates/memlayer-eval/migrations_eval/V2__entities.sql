-- Generated with AI Coding Rules Hub
-- V2__entities.sql — eval-side entity store for the P2 entity layer.
--
-- Adds the three tables Mem0's audit (spec §3) showed are the second-largest
-- accuracy contributor: an `entities` registry keyed by (project, name), an
-- `entity_links` join table to facts, and an `entities_vec` virtual table for
-- semantic entity matching at query time. Together they support SC-12
-- (entity store populated) and SC-13 (additive scoring with entity boost).
--
-- This migration applies on top of V1__facts.sql; the two coexist in the
-- eval-only migration root at crates/memlayer-eval/migrations_eval/.
-- memlayer-storage is unaffected (SC-10).
--
-- Spec: retrieval-upgrade-v1 §5. Plan: P2 §4.5 (Mem0 audit).

-- entities ------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS entities (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project      TEXT NOT NULL,
    name         TEXT NOT NULL,             -- canonical lowercased entity string
    kind         TEXT,                      -- "person" / "place" / "thing" / NULL
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(project, name)
);

-- entity_links --------------------------------------------------------------
-- Many-to-many: each entity may link to several facts; each fact may surface
-- several entities. Cascades aren't strictly necessary because the eval
-- pipeline rebuilds facts.db from scratch on `eval extract` re-run, but
-- declaring FKs documents the relationship for any future maintenance hook.
CREATE TABLE IF NOT EXISTS entity_links (
    entity_id    INTEGER NOT NULL,
    fact_id      INTEGER NOT NULL,
    PRIMARY KEY(entity_id, fact_id),
    FOREIGN KEY (entity_id) REFERENCES entities(id),
    FOREIGN KEY (fact_id)   REFERENCES facts(id)
);
CREATE INDEX IF NOT EXISTS idx_entity_links_fact ON entity_links(fact_id);

-- entities_vec --------------------------------------------------------------
-- 384-dim BGE-small embeddings keyed by entities.id (vec_rowid).
-- Created here only if the sqlite-vec extension is loaded; FactsDb::open
-- enforces that order (load extension -> run migrations).
CREATE VIRTUAL TABLE IF NOT EXISTS entities_vec USING vec0(embedding float[384]);

-- bump schema_meta.version --------------------------------------------------
UPDATE schema_meta SET value = '2' WHERE key = 'version';
