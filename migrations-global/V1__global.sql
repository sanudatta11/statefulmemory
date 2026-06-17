-- V1__global.sql — global cross-project mirror schema.
--
-- Stored at `~/.memlayer/global.sqlite` (separate from per-project DBs at
-- `~/.memlayer/projects/<id>.db`). The daemon mirrors every successful
-- `save_observation` into this DB so that `obs search --all-projects`
-- can serve a deduped, BM25-ranked view across every memlayer-tracked
-- repo without fan-out at query time.
--
-- Observations are *mirrored* from per-project DBs — they retain their
-- per-project `id` as `source_id` here. The unique (project, source_id)
-- constraint allows idempotent re-mirroring (INSERT OR REPLACE).

CREATE TABLE IF NOT EXISTS observations (
    global_id   INTEGER PRIMARY KEY AUTOINCREMENT,
    project     TEXT NOT NULL,
    source_id   INTEGER NOT NULL,
    type        TEXT NOT NULL,
    title       TEXT NOT NULL,
    content     TEXT NOT NULL,
    topic_key   TEXT,
    created_at  TEXT NOT NULL,
    UNIQUE(project, source_id)
);

CREATE INDEX IF NOT EXISTS idx_global_obs_project ON observations(project);
CREATE INDEX IF NOT EXISTS idx_global_obs_topic   ON observations(topic_key) WHERE topic_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_global_obs_created ON observations(created_at DESC);

-- FTS5 virtual table mirrors the title + content for global BM25 queries.
CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    title, content, type, topic_key,
    content='observations', content_rowid='global_id'
);

CREATE TRIGGER IF NOT EXISTS global_obs_fts_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, title, content, type, topic_key)
    VALUES (new.global_id, new.title, new.content, new.type, new.topic_key);
END;

CREATE TRIGGER IF NOT EXISTS global_obs_fts_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, content, type, topic_key)
    VALUES ('delete', old.global_id, old.title, old.content, old.type, old.topic_key);
END;

CREATE TRIGGER IF NOT EXISTS global_obs_fts_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, content, type, topic_key)
    VALUES ('delete', old.global_id, old.title, old.content, old.type, old.topic_key);
    INSERT INTO observations_fts(rowid, title, content, type, topic_key)
    VALUES (new.global_id, new.title, new.content, new.type, new.topic_key);
END;

-- manifest: per-project sync state for the global mirror.
CREATE TABLE IF NOT EXISTS manifest (
    project              TEXT PRIMARY KEY,
    last_synced_at       TEXT NOT NULL,
    observation_count    INTEGER NOT NULL DEFAULT 0
);
