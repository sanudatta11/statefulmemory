-- V6__embeddings_quantize.sql — add int8 quantized storage to observation_embedding_meta.
--
-- Two new optional columns on observation_embedding_meta allow the embed worker
-- to store a compact 384-byte int8 representation (instead of the 1,536-byte
-- float32 blob in observations_vec) when EmbedConfig.quantize = true.
--
-- The quantized path skips observations_vec entirely; the daemon's search_dense
-- brute-forces cosine similarity directly from these columns.
--
-- Idempotent: IF NOT EXISTS / default NULL means re-running is a no-op.
ALTER TABLE observation_embedding_meta ADD COLUMN quantized_blob BLOB;
ALTER TABLE observation_embedding_meta ADD COLUMN scale REAL;
