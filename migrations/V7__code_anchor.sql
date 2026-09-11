-- V7__code_anchor.sql — code anchor column for linking observations to symbols/files
ALTER TABLE observations ADD COLUMN code_anchor TEXT;

CREATE INDEX IF NOT EXISTS idx_observations_code_anchor
    ON observations(code_anchor);
