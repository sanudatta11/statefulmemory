-- V2__export_tracking.sql — add export tracking columns + partial indexes.
--
-- Source: Spec 3 (sync-and-export) §FR1.1, §FR6.1, §FR6.2, SC-2, SC-16.
--
-- Forward-only. `ADD COLUMN` is non-destructive: existing rows receive
-- `exported_at = NULL`, which is exactly what `select_unexported_for_export`
-- treats as "not yet exported" (FR6.2, SC-16). Partial indexes only cover
-- NULL rows so they stay tiny once data has been exported (NFR5).

-- exported_at columns -------------------------------------------------------

ALTER TABLE observations ADD COLUMN exported_at TEXT;
ALTER TABLE sessions     ADD COLUMN exported_at TEXT;
ALTER TABLE user_prompts ADD COLUMN exported_at TEXT;

-- partial indexes for fast "what's left to export" scans --------------------

CREATE INDEX IF NOT EXISTS idx_obs_unexported     ON observations(exported_at) WHERE exported_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_sess_unexported    ON sessions(exported_at)     WHERE exported_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_prompts_unexported ON user_prompts(exported_at) WHERE exported_at IS NULL;

-- bump schema_meta.version --------------------------------------------------

UPDATE schema_meta SET value = '2' WHERE key = 'version';
