-- Generated with AI Coding Rules Hub
-- V4__session_tier.sql — session-level summary tier (P5 spec-task-27d).
--
-- Adds a `tier` column to `facts` so the extraction pipeline can write
-- session-level summary facts alongside the per-window facts. Defaults
-- to 'window' for backward compatibility — existing rows are
-- automatically tagged. Session summaries are written as
-- (subject=project, predicate='session_summary', tier='session') with
-- salience = mean of the top-5 session facts.
--
-- LongMemEval especially benefits from session-tier facts because its
-- queries often span multi-session context that no single window
-- captures cleanly.
--
-- Spec link: SC-13. Plan: P5 spec-task-27d.

ALTER TABLE facts ADD COLUMN tier TEXT NOT NULL DEFAULT 'window';

CREATE INDEX IF NOT EXISTS idx_facts_tier ON facts(project, tier);

UPDATE schema_meta SET value = '4' WHERE key = 'version';
