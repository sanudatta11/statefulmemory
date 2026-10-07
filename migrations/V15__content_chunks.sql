-- V15__content_chunks.sql — multi-vector chunking for long content (Phase 2.5).
--
-- Today the whole-observation `{title} {content}` string is embedded as ONE
-- 384-dim vector (embed_worker.rs); content longer than BGE-small's ~512
-- token window gets mean-pool-diluted. This adds an OPT-IN side-path: long
-- content is also split into chunks, each chunk gets its own vector, and
-- dense search can max-pool (best chunk) similarity back to the parent
-- observation. The whole-document vector (`observations_vec` /
-- `observations_vec_i8`, V4/V14) is UNCHANGED and still written for every
-- observation — chunk vectors are additive, not a replacement.
--
-- `observations_chunks` mirrors the `facts` cascade pattern (V5): a real
-- table with `ON DELETE CASCADE` from `observations`, and its own AFTER
-- DELETE trigger that cleans up the paired vec0 row (virtual tables can't be
-- FK targets, same reasoning as the V4 `observations_vec` trigger).
--
-- Idempotent: IF NOT EXISTS everywhere.

CREATE TABLE IF NOT EXISTS observations_chunks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    obs_id      INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
    chunk_idx   INTEGER NOT NULL,
    text        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_observations_chunks_obs_id
    ON observations_chunks(obs_id);

-- Shadow vec0 table keyed by observations_chunks.id (same 1:1-by-rowid
-- pattern as observations_vec <-> observations.id in V4).
--
-- NAMING: must NOT be named "observations_vec_chunks" — sqlite-vec's vec0
-- module auto-creates an INTERNAL shadow table named "<vec0-table>_chunks"
-- for its own storage bookkeeping, so "observations_vec" (V4) already owns
-- the name "observations_vec_chunks" as its shadow table. Reusing it would
-- make this CREATE VIRTUAL TABLE IF NOT EXISTS silently no-op against the
-- wrong (pre-existing, incompatible) table. Named "chunk_vectors" instead,
-- which cannot collide with any vec0 shadow-table suffix of an existing table.
CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
    embedding float[384]
);

CREATE TRIGGER IF NOT EXISTS observations_chunks_after_delete_vec
AFTER DELETE ON observations_chunks
BEGIN
    DELETE FROM chunk_vectors WHERE rowid = OLD.id;
END;
