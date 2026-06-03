-- V1__init.sql — initial schema for a per-project memlayer database.
--
-- Source: PRD §5.3 (verbatim). Spec 1 references §12 of core-storage-daemon spec.
--
-- Run on first connect to a fresh `~/.memlayer/projects/<normalized>.db`. After
-- this migration, `schema_meta.version` = 1 and the FTS5 virtual tables are
-- ready to receive triggers' insert/delete fan-out.

-- sessions ------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    directory  TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at   TEXT,
    summary    TEXT
);

-- observations + indexes ----------------------------------------------------

CREATE TABLE IF NOT EXISTS observations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_id         TEXT NOT NULL UNIQUE,
    session_id      TEXT NOT NULL,
    type            TEXT NOT NULL,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL,
    tool_name       TEXT,
    scope           TEXT NOT NULL DEFAULT 'project',
    created_by      TEXT,
    topic_key       TEXT,
    normalized_hash TEXT,
    revision_count  INTEGER NOT NULL DEFAULT 1,
    duplicate_count INTEGER NOT NULL DEFAULT 1,
    last_seen_at    TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at      TEXT,
    review_after    TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_obs_session     ON observations(session_id);
CREATE INDEX IF NOT EXISTS idx_obs_type        ON observations(type);
CREATE INDEX IF NOT EXISTS idx_obs_created     ON observations(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_obs_scope       ON observations(scope);
CREATE INDEX IF NOT EXISTS idx_obs_created_by  ON observations(created_by) WHERE created_by IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_obs_topic       ON observations(topic_key, scope, updated_at DESC) WHERE topic_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_obs_dedupe      ON observations(normalized_hash, created_at DESC) WHERE normalized_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_obs_active      ON observations(deleted_at) WHERE deleted_at IS NULL;

-- observations_fts ----------------------------------------------------------

CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    title, content, tool_name, type, topic_key,
    content='observations', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS obs_fts_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, title, content, tool_name, type, topic_key)
    VALUES (new.id, new.title, new.content, new.tool_name, new.type, new.topic_key);
END;

CREATE TRIGGER IF NOT EXISTS obs_fts_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, content, tool_name, type, topic_key)
    VALUES ('delete', old.id, old.title, old.content, old.tool_name, old.type, old.topic_key);
END;

CREATE TRIGGER IF NOT EXISTS obs_fts_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, content, tool_name, type, topic_key)
    VALUES ('delete', old.id, old.title, old.content, old.tool_name, old.type, old.topic_key);
    INSERT INTO observations_fts(rowid, title, content, tool_name, type, topic_key)
    VALUES (new.id, new.title, new.content, new.tool_name, new.type, new.topic_key);
END;

-- user_prompts + FTS --------------------------------------------------------

CREATE TABLE IF NOT EXISTS user_prompts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_id    TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    content    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_prompts_session ON user_prompts(session_id);
CREATE INDEX IF NOT EXISTS idx_prompts_created ON user_prompts(created_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
    content, content='user_prompts', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS prompt_fts_ai AFTER INSERT ON user_prompts BEGIN
    INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS prompt_fts_ad AFTER DELETE ON user_prompts BEGIN
    INSERT INTO prompts_fts(prompts_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS prompt_fts_au AFTER UPDATE ON user_prompts BEGIN
    INSERT INTO prompts_fts(prompts_fts, rowid, content) VALUES ('delete', old.id, old.content);
    INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;

-- sync_chunks (used by Spec 3; created here for forward-compat) -------------

CREATE TABLE IF NOT EXISTS sync_chunks (
    chunk_id    TEXT PRIMARY KEY,
    imported_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- schema_meta ---------------------------------------------------------------

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('version', '1');
