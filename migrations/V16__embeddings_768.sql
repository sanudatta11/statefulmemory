-- V16__embeddings_768.sql — parallel vec0 schema for a 768-dim embedding
-- model tier (Phase 6.3: embedding model upgrade, e.g. bge-base-en-v1.5 /
-- bge-m3-small configs that report hidden_size=768).
--
-- sqlite-vec's vec0 virtual table has a FIXED dimension baked in at
-- CREATE TABLE time, and refinery migrations are static SQL embedded at
-- compile time — there is no way to parameterize "float[?]" by runtime
-- config. So a model with a different output dimension can't reuse the V4
-- (float[384]) / V14 (int8[384]) tables; it needs its own fixed-dimension
-- table set. This mirrors exactly how V14 added a second (int8) table
-- alongside V4's float table, rather than trying to make one vec0 table
-- serve two encodings.
--
-- The daemon selects which table set to read/write based on the ACTUAL
-- embedding length produced by the configured embedder (see
-- storage::write::handle_insert_embedding and storage::read::search_dense):
-- 384 -> observations_vec / observations_vec_i8 (V4 / V14); 768 -> these.
-- A project only ever has rows in one dimension's tables at a time —
-- `pragmas::check_embed_model_compat` already refuses to start the daemon
-- against a project whose stored model/dim doesn't match the configured one,
-- so there's no risk of silently mixing dimensions within a project.
--
-- Idempotent: IF NOT EXISTS everywhere.

CREATE VIRTUAL TABLE IF NOT EXISTS observations_vec_768 USING vec0(
    embedding float[768]
);

CREATE VIRTUAL TABLE IF NOT EXISTS observations_vec_768_i8 USING vec0(
    embedding int8[768] distance_metric=cosine
);

CREATE TRIGGER IF NOT EXISTS observations_after_delete_vec_768
AFTER DELETE ON observations
BEGIN
    DELETE FROM observations_vec_768 WHERE rowid = OLD.id;
    DELETE FROM observations_vec_768_i8 WHERE rowid = OLD.id;
END;
