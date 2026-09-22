-- V12__fact_key_expand.sql — LongMemEval index-time fact-augmented keys
-- Shadow column indexed by FTS; observations.content stays display-clean.

ALTER TABLE observations ADD COLUMN key_expand TEXT NOT NULL DEFAULT '';

DROP TRIGGER IF EXISTS obs_fts_ai;
DROP TRIGGER IF EXISTS obs_fts_ad;
DROP TRIGGER IF EXISTS obs_fts_au;
DROP TABLE IF EXISTS observations_fts;

CREATE VIRTUAL TABLE observations_fts USING fts5(
    title, content, tool_name, type, topic_key, key_expand,
    content='observations', content_rowid='id'
);

CREATE TRIGGER obs_fts_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, title, content, tool_name, type, topic_key, key_expand)
    VALUES (new.id, new.title, new.content, new.tool_name, new.type, new.topic_key, new.key_expand);
END;

CREATE TRIGGER obs_fts_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, content, tool_name, type, topic_key, key_expand)
    VALUES ('delete', old.id, old.title, old.content, old.tool_name, old.type, old.topic_key, old.key_expand);
END;

CREATE TRIGGER obs_fts_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, title, content, tool_name, type, topic_key, key_expand)
    VALUES ('delete', old.id, old.title, old.content, old.tool_name, old.type, old.topic_key, old.key_expand);
    INSERT INTO observations_fts(rowid, title, content, tool_name, type, topic_key, key_expand)
    VALUES (new.id, new.title, new.content, new.tool_name, new.type, new.topic_key, new.key_expand);
END;

INSERT INTO observations_fts(observations_fts) VALUES('rebuild');
