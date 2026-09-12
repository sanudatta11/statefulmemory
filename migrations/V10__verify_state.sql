-- V10__verify_state.sql — freshness state, tracked independently of confidence.
ALTER TABLE observations ADD COLUMN verify_state TEXT NOT NULL DEFAULT 'unanchored';
ALTER TABLE observations ADD COLUMN verified_commit TEXT;
ALTER TABLE observations ADD COLUMN verified_at TEXT;

CREATE INDEX IF NOT EXISTS idx_observations_verify_state
    ON observations(verify_state);
