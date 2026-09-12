-- V9__observation_anchors.sql — multi-anchor table binding observations to
-- repository artifacts, with the commit and content digest used at write time.
CREATE TABLE IF NOT EXISTS observation_anchors (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    observation_id  INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
    path            TEXT NOT NULL,
    symbol          TEXT,
    line_start      INTEGER,
    line_end        INTEGER,
    anchor_commit   TEXT,
    content_digest  TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(observation_id, path, symbol)
);

CREATE INDEX IF NOT EXISTS idx_obs_anchors_obs  ON observation_anchors(observation_id);
CREATE INDEX IF NOT EXISTS idx_obs_anchors_path ON observation_anchors(path);
