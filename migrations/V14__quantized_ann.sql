-- V14__quantized_ann.sql — ANN-backed int8 quantized vectors (Phase 6.2).
--
-- Today's `quantize = true` mode stores int8 blobs in
-- `observation_embedding_meta.quantized_blob` and the daemon falls back to a
-- brute-force Rust cosine scan over every row (storage::read::search_dense_quantized)
-- — an O(N) scaling cliff. sqlite-vec 0.1.9 (vendored) natively supports an
-- `int8[dim]` vec0 column with `distance_metric=cosine`, which is scale-invariant
-- per vector (cosine cancels the per-vector scale in the dot/magnitude ratio),
-- so the EXISTING per-vector `calibrate_scale` quantization in
-- `statefulmemory_embed::quantize` is safe to feed into this ANN index unchanged.
--
-- `observations_vec_i8` is populated going forward by the write path when
-- `embed.quantize = true`; existing `quantized_blob` rows are backfilled by
-- `statefulmemory reindex` (re-embeds and writes to this table instead of the
-- legacy blob-only path). The legacy `quantized_blob`/`scale` columns and the
-- brute-force scan remain as a fallback when this table is empty for a project
-- (e.g. before a reindex).
--
-- Idempotent: IF NOT EXISTS everywhere, so re-running on an upgraded DB is a no-op.

CREATE VIRTUAL TABLE IF NOT EXISTS observations_vec_i8 USING vec0(
    embedding int8[384] distance_metric=cosine
);

-- Cascade-delete the ANN row when an observation is hard-deleted, mirroring
-- the float vec0 trigger from V4.
CREATE TRIGGER IF NOT EXISTS observations_after_delete_vec_i8
AFTER DELETE ON observations
BEGIN
    DELETE FROM observations_vec_i8 WHERE rowid = OLD.id;
END;
