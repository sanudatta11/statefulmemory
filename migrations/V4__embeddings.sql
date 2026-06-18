-- V4__embeddings.sql — vector storage for hybrid retrieval (SC-2, SC-10, SC-11).
--
-- Adds the `vec0` virtual table that stores 384-dim BGE-small embeddings, and
-- a sidecar `observation_embedding_meta` table tracking which model produced
-- each row. Cascade delete keeps the two in sync when an observation goes away.
--
-- The vec0 module is provided by sqlite-vec, registered as a process-global
-- auto-extension via pragmas::ensure_sqlite_vec_extension(); migrations run
-- after that, so vec0 is available here.
--
-- Idempotent: every statement uses `IF NOT EXISTS`, so re-running this
-- migration on an already-upgraded DB is a no-op (refinery normally guards
-- this anyway, but we belt-and-suspenders for SC-10).

CREATE VIRTUAL TABLE IF NOT EXISTS observations_vec USING vec0(
    embedding float[384]
);

CREATE TABLE IF NOT EXISTS observation_embedding_meta (
    observation_id  INTEGER PRIMARY KEY REFERENCES observations(id) ON DELETE CASCADE,
    model           TEXT NOT NULL,
    dim             INTEGER NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    quantized       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_obs_embed_meta_model
    ON observation_embedding_meta(model);

-- Cascade-delete the vec row when an observation is hard-deleted. SQLite
-- foreign keys can't span virtual tables, so we use a trigger.
CREATE TRIGGER IF NOT EXISTS observations_after_delete_vec
AFTER DELETE ON observations
BEGIN
    DELETE FROM observations_vec WHERE rowid = OLD.id;
END;
