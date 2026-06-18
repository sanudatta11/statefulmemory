-- V5__facts.sql — atomic-fact store for retrieval promotion (SC-8, SC-9, SC-10).
--
-- Each fact is a (subject, predicate, object) triple extracted from an
-- observation by Haiku or Sonnet. The fact lives until its source observation
-- is deleted (cascade) or until a newer fact supersedes it; supersession is
-- modeled but NOT enforced in this spec — readers filter by
-- `superseded_by IS NULL` for "active" facts.
--
-- Idempotent: every statement uses IF NOT EXISTS so re-running this migration
-- is a no-op (SC-10).

CREATE TABLE IF NOT EXISTS facts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    obs_id          INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
    subject         TEXT NOT NULL,
    predicate       TEXT NOT NULL,
    object          TEXT NOT NULL,
    temporal        TEXT,
    salience        REAL NOT NULL DEFAULT 0.5,
    superseded_by   INTEGER REFERENCES facts(id),
    extracted_by    TEXT NOT NULL,                            -- "haiku" | "sonnet"
    extracted_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_facts_obs       ON facts(obs_id);
CREATE INDEX IF NOT EXISTS idx_facts_subject   ON facts(subject);
CREATE INDEX IF NOT EXISTS idx_facts_active    ON facts(superseded_by) WHERE superseded_by IS NULL;
CREATE INDEX IF NOT EXISTS idx_facts_predicate ON facts(predicate);

-- facts_fts: BM25 search over the (subject, predicate, object) triple.
-- content='facts' makes this a contentless FTS that mirrors the base table.
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    subject, predicate, object,
    content='facts', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS facts_fts_ai AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, subject, predicate, object)
    VALUES (new.id, new.subject, new.predicate, new.object);
END;

CREATE TRIGGER IF NOT EXISTS facts_fts_ad AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, predicate, object)
    VALUES ('delete', old.id, old.subject, old.predicate, old.object);
END;

CREATE TRIGGER IF NOT EXISTS facts_fts_au AFTER UPDATE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, predicate, object)
    VALUES ('delete', old.id, old.subject, old.predicate, old.object);
    INSERT INTO facts_fts(rowid, subject, predicate, object)
    VALUES (new.id, new.subject, new.predicate, new.object);
END;
