-- V11__graph_briefing.sql — cue-entity graph for briefing assembly (spec: graph-briefing)
CREATE TABLE IF NOT EXISTS entities (
    id        INTEGER PRIMARY KEY,
    kind      TEXT NOT NULL CHECK (kind IN ('file','symbol','concept','person','agent')),
    name      TEXT NOT NULL,
    norm_name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS entity_mentions (
    entity_id      INTEGER NOT NULL REFERENCES entities(id),
    observation_id INTEGER NOT NULL,
    offsets        TEXT,
    source         TEXT NOT NULL CHECK (source IN ('anchor','backtick','topic','token'))
);
CREATE INDEX IF NOT EXISTS idx_entity_mentions_obs ON entity_mentions(observation_id);
CREATE INDEX IF NOT EXISTS idx_entity_mentions_ent ON entity_mentions(entity_id);

CREATE TABLE IF NOT EXISTS entity_edges (
    id                 INTEGER PRIMARY KEY,
    from_entity        INTEGER NOT NULL REFERENCES entities(id),
    to_entity          INTEGER NOT NULL REFERENCES entities(id),
    relation           TEXT NOT NULL CHECK (relation IN ('mentions','fixes','contradicts','about','co_occurs')),
    weight             REAL NOT NULL DEFAULT 1.0,
    first_seen         INTEGER,
    last_seen          INTEGER,
    src_observation_id INTEGER
);
CREATE INDEX IF NOT EXISTS idx_entity_edges_from ON entity_edges(from_entity);
CREATE INDEX IF NOT EXISTS idx_entity_edges_to   ON entity_edges(to_entity);
