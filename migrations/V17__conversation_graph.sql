-- V17__conversation_graph.sql — provenance-carrying heterogeneous memory graph.
-- Existing entity tables remain the compatibility surface; these projections
-- make sessions, observations, and facts traversable without duplicating their
-- authoritative content.

CREATE TABLE IF NOT EXISTS graph_nodes (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('session','observation','fact','entity','event')),
    ref_key       TEXT NOT NULL,
    label         TEXT NOT NULL,
    norm_key      TEXT NOT NULL,
    valid_from    TEXT,
    valid_to      TEXT,
    recorded_at   TEXT NOT NULL DEFAULT (datetime('now')),
    confidence    REAL NOT NULL DEFAULT 1.0,
    source        TEXT NOT NULL DEFAULT 'deterministic',
    UNIQUE(kind, ref_key)
);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind_norm ON graph_nodes(kind, norm_key);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_ref ON graph_nodes(ref_key);

CREATE TABLE IF NOT EXISTS graph_edges (
    id                    INTEGER PRIMARY KEY,
    from_node             INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    to_node               INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    relation              TEXT NOT NULL,
    weight                REAL NOT NULL DEFAULT 1.0,
    confidence            REAL NOT NULL DEFAULT 1.0,
    valid_from            TEXT,
    valid_to              TEXT,
    recorded_at           TEXT NOT NULL DEFAULT (datetime('now')),
    source                 TEXT NOT NULL DEFAULT 'deterministic',
    src_observation_id    INTEGER,
    source_span            TEXT,
    metadata_json          TEXT,
    UNIQUE(from_node, to_node, relation, src_observation_id)
);
CREATE INDEX IF NOT EXISTS idx_graph_edges_from_rel ON graph_edges(from_node, relation);
CREATE INDEX IF NOT EXISTS idx_graph_edges_to_rel ON graph_edges(to_node, relation);
CREATE INDEX IF NOT EXISTS idx_graph_edges_obs ON graph_edges(src_observation_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_valid ON graph_edges(valid_from, valid_to);

-- Project the existing rows into the heterogeneous graph. INSERT OR IGNORE
-- keeps upgrades safe for partially rebuilt databases.
INSERT OR IGNORE INTO graph_nodes(kind, ref_key, label, norm_key, confidence, source)
SELECT 'entity', 'entity:' || id, name, norm_name, 1.0, 'deterministic' FROM entities;
INSERT OR IGNORE INTO graph_nodes(kind, ref_key, label, norm_key, recorded_at, confidence, source)
SELECT 'session', 'session:' || id, id, lower(id), started_at, 1.0, 'deterministic' FROM sessions;
INSERT OR IGNORE INTO graph_nodes(kind, ref_key, label, norm_key, recorded_at, confidence, source)
SELECT 'observation', 'observation:' || id, title, lower(title), created_at, 1.0, 'deterministic' FROM observations;
INSERT OR IGNORE INTO graph_nodes(kind, ref_key, label, norm_key, recorded_at, confidence, source)
SELECT 'fact', 'fact:' || id, subject || ' ' || predicate || ' ' || object,
       lower(subject || '|' || predicate || '|' || object), extracted_at,
       CASE WHEN salience BETWEEN 0 AND 1 THEN salience ELSE 0.5 END,
       'deterministic' FROM facts;

INSERT OR IGNORE INTO graph_edges(from_node, to_node, relation, weight, confidence, recorded_at, source)
SELECT s.id, o.id, 'contains', 1.0, 1.0, o.recorded_at, 'deterministic'
FROM graph_nodes s JOIN sessions ss ON s.kind='session' AND s.ref_key='session:' || ss.id
JOIN observations ob ON ob.session_id = ss.id
JOIN graph_nodes o ON o.kind='observation' AND o.ref_key='observation:' || ob.id;

INSERT OR IGNORE INTO graph_edges(from_node, to_node, relation, weight, confidence, recorded_at, source, src_observation_id)
SELECT f.id, o.id, 'derived_from', 1.0, 1.0, f.recorded_at, 'deterministic', CAST(substr(o.ref_key, 14) AS INTEGER)
FROM graph_nodes f JOIN facts fa ON f.kind='fact' AND f.ref_key='fact:' || fa.id
JOIN graph_nodes o ON o.kind='observation' AND o.ref_key='observation:' || fa.obs_id;

INSERT OR IGNORE INTO graph_edges(from_node, to_node, relation, weight, confidence, recorded_at, source, src_observation_id)
SELECT en.id, eo.id, 'mentions', 1.0, 1.0, eo.recorded_at, 'deterministic', em.observation_id
FROM entity_mentions em
JOIN graph_nodes en ON en.kind='entity' AND en.ref_key='entity:' || em.entity_id
JOIN graph_nodes eo ON eo.kind='observation' AND eo.ref_key='observation:' || em.observation_id;

INSERT OR IGNORE INTO graph_edges(from_node, to_node, relation, weight, confidence, recorded_at, source, src_observation_id)
SELECT ef.id, et.id, ee.relation, ee.weight, 1.0, COALESCE(datetime(ee.last_seen, 'unixepoch'), datetime('now')), 'deterministic', ee.src_observation_id
FROM entity_edges ee
JOIN graph_nodes ef ON ef.kind='entity' AND ef.ref_key='entity:' || ee.from_entity
JOIN graph_nodes et ON et.kind='entity' AND et.ref_key='entity:' || ee.to_entity;

UPDATE schema_meta SET value = '17' WHERE key = 'version';
