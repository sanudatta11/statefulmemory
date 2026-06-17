-- V6__conflict.sql — add supersession tracking to observations.
--
-- Allows a newer observation to mark an older one as superseded at save time,
-- preventing conflicting observations from both appearing in obs context.

ALTER TABLE observations ADD COLUMN superseded_by_id INTEGER REFERENCES observations(id);
ALTER TABLE observations ADD COLUMN delete_reason    TEXT;
ALTER TABLE observations ADD COLUMN superseded_count INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_obs_superseded ON observations(superseded_by_id) WHERE superseded_by_id IS NOT NULL;
