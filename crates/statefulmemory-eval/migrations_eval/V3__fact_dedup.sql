-- Generated with AI Coding Rules Hub
-- V3__fact_dedup.sql — canonical key dedup (P5 spec-task-27b).
--
-- Pre-P5 the extractor produced duplicate facts whenever the same
-- (subject, predicate) showed up in overlapping windows. The additive
-- scorer surfaced all copies, wasting top-k slots and inflating the
-- prompt. This migration:
--
--   1. Dedups existing rows: for every (project, lower(subject||predicate))
--      group with >1 row, keep the highest-salience member (id-asc tiebreak)
--      and delete the rest. Orphaned entity_links and facts_vec rows are
--      cleaned in lockstep.
--   2. Adds a UNIQUE INDEX on (project, canonical) so future bulk writes
--      can use ON CONFLICT for idempotent re-extraction.
--
-- Spec link: SC-13 (additive scoring quality), retrieval-upgrade-v1 §6.
-- Plan: P5 spec-task-27b.

-- Step 1: drop entity_links pointing at facts that will be deleted.
DELETE FROM entity_links WHERE fact_id IN (
    SELECT a.id FROM facts a
    WHERE EXISTS (
        SELECT 1 FROM facts b
        WHERE b.project = a.project
          AND lower(b.subject || '|' || b.predicate) = lower(a.subject || '|' || a.predicate)
          AND (b.salience > a.salience OR (b.salience = a.salience AND b.id < a.id))
    )
);

-- Step 2: delete the lower-salience duplicates from facts.
DELETE FROM facts WHERE id IN (
    SELECT a.id FROM facts a
    WHERE EXISTS (
        SELECT 1 FROM facts b
        WHERE b.project = a.project
          AND lower(b.subject || '|' || b.predicate) = lower(a.subject || '|' || a.predicate)
          AND (b.salience > a.salience OR (b.salience = a.salience AND b.id < a.id))
    )
);

-- Step 3: drop facts_vec rows that no longer have a fact (vec0 is a
-- separate table; the FTS triggers handle facts_fts in step 4).
DELETE FROM facts_vec WHERE rowid NOT IN (SELECT id FROM facts);

-- Step 4: facts_fts is content-rowid'd to facts; SQLite's FTS5 cleanup
-- happens automatically when facts rows are deleted via the AFTER DELETE
-- trigger declared in V1__facts.sql. No explicit cleanup needed here.

-- Step 5: enforce canonical-key uniqueness going forward. Expression-
-- index is supported by SQLite's UPSERT path (matches the conflict
-- target in facts_writer's ON CONFLICT clause).
CREATE UNIQUE INDEX IF NOT EXISTS idx_facts_canonical
    ON facts(project, lower(subject || '|' || predicate));

-- Step 6: bump schema_meta version.
UPDATE schema_meta SET value = '3' WHERE key = 'version';
