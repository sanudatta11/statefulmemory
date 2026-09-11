-- V8__observation_relations.sql — observation graph relation edges (conflicts_with, supersedes, scoped_by)
CREATE TABLE IF NOT EXISTS observation_relations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id       INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
    target_id       INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL,
    confidence      REAL NOT NULL DEFAULT 1.0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_id, target_id, relation_type)
);

CREATE INDEX IF NOT EXISTS idx_obs_rel_source ON observation_relations(source_id);
CREATE INDEX IF NOT EXISTS idx_obs_rel_target ON observation_relations(target_id);
