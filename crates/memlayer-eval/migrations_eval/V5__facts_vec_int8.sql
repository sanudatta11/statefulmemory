-- Generated with AI Coding Rules Hub
-- V5__facts_vec_int8.sql — int8 vector path for BEAM scale (P4 spec-task-24).
--
-- Writes are gated by MEMLAYER_VEC_INT8=1 in facts_writer. When the env
-- var is unset, embeddings continue to land in `facts_vec` (float[384])
-- and this table stays empty — preserving full recall on
-- LoCoMo / LongMemEval. When set, writes go here instead, cutting
-- 10M × 384 × 4B (15 GB f32) → 10M × 384 × 1B (3.8 GB int8).
--
-- Recall validated by tests/quantize_recall.rs (TS-12): top-10 Jaccard
-- vs f32 baseline ≥ 0.9 on 1000 vectors / 50 sample queries.
--
-- Spec link: SC-NFR-MEM. Plan: P4 spec-task-24.

CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec_int8 USING vec0(embedding int8[384]);

UPDATE schema_meta SET value = '5' WHERE key = 'version';
