//! Heterogeneous conversation/fact/entity graph projection.
//!
//! The source tables remain authoritative. This module stores typed node and
//! edge projections with provenance so graph retrieval can explain every lift.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use statefulmemory_core::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNode {
    pub id: i64,
    pub kind: String,
    pub ref_key: String,
    pub label: String,
    pub norm_key: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryGraphEdge {
    pub id: i64,
    pub from_node: i64,
    pub to_node: i64,
    pub relation: String,
    pub weight: f64,
    pub confidence: f64,
    pub src_observation_id: Option<i64>,
    pub source: String,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphPath {
    pub node_ids: Vec<i64>,
    pub edge_ids: Vec<i64>,
    pub score: f64,
    pub hops: u8,
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_node(
    conn: &Connection,
    kind: &str,
    ref_key: &str,
    label: &str,
    norm_key: &str,
    recorded_at: Option<&str>,
    confidence: f64,
    source: &str,
) -> Result<i64> {
    conn.query_row(
        "INSERT INTO graph_nodes(kind, ref_key, label, norm_key, recorded_at, confidence, source)
         VALUES (?1, ?2, ?3, ?4, COALESCE(?5, datetime('now')), ?6, ?7)
         ON CONFLICT(kind, ref_key) DO UPDATE SET
           label=excluded.label, norm_key=excluded.norm_key,
           recorded_at=COALESCE(excluded.recorded_at, graph_nodes.recorded_at),
           confidence=MAX(graph_nodes.confidence, excluded.confidence)
         RETURNING id",
        params![
            kind,
            ref_key,
            label,
            norm_key,
            recorded_at,
            confidence,
            source
        ],
        |row| row.get(0),
    )
    .map_err(|e| Error::internal(format!("upsert graph node: {e}")))
}

#[allow(clippy::too_many_arguments)]
pub fn insert_edge(
    conn: &Connection,
    from_node: i64,
    to_node: i64,
    relation: &str,
    weight: f64,
    confidence: f64,
    valid_from: Option<&str>,
    valid_to: Option<&str>,
    source: &str,
    src_observation_id: Option<i64>,
    source_span: Option<&str>,
    metadata_json: Option<&str>,
) -> Result<i64> {
    if from_node == to_node {
        return Ok(0);
    }
    let existing: Option<i64> = match src_observation_id {
        Some(source_id) => conn
            .query_row(
                "SELECT id FROM graph_edges WHERE from_node=?1 AND to_node=?2 AND relation=?3 AND src_observation_id=?4",
                params![from_node, to_node, relation, source_id], |row| row.get(0),
            )
            .optional(),
        None => conn
            .query_row(
                "SELECT id FROM graph_edges WHERE from_node=?1 AND to_node=?2 AND relation=?3 AND src_observation_id IS NULL",
                params![from_node, to_node, relation], |row| row.get(0),
            )
            .optional(),
    }
    .map_err(|e| Error::internal(format!("lookup graph edge: {e}")))?;
    if let Some(id) = existing {
        conn.execute(
            "UPDATE graph_edges SET weight=weight+?2, confidence=MAX(confidence,?3),
                valid_from=COALESCE(valid_from,?4), valid_to=COALESCE(?5,valid_to),
                source_span=COALESCE(?6,source_span), metadata_json=COALESCE(?7,metadata_json)
             WHERE id=?1",
            params![
                id,
                weight,
                confidence,
                valid_from,
                valid_to,
                source_span,
                metadata_json
            ],
        )
        .map_err(|e| Error::internal(format!("update graph edge: {e}")))?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO graph_edges
            (from_node,to_node,relation,weight,confidence,valid_from,valid_to,source,
             src_observation_id,source_span,metadata_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            from_node,
            to_node,
            relation,
            weight,
            confidence,
            valid_from,
            valid_to,
            source,
            src_observation_id,
            source_span,
            metadata_json
        ],
    )
    .map_err(|e| Error::internal(format!("insert graph edge: {e}")))?;
    Ok(conn.last_insert_rowid())
}

pub fn node_by_ref(conn: &Connection, kind: &str, ref_key: &str) -> Result<Option<GraphNode>> {
    conn.query_row(
        "SELECT id,kind,ref_key,label,norm_key,confidence FROM graph_nodes WHERE kind=?1 AND ref_key=?2",
        params![kind, ref_key],
        |r| Ok(GraphNode { id:r.get(0)?, kind:r.get(1)?, ref_key:r.get(2)?, label:r.get(3)?, norm_key:r.get(4)?, confidence:r.get(5)? }),
    ).optional().map_err(|e| Error::internal(format!("lookup graph node: {e}")))
}

pub fn nodes_by_norm(conn: &Connection, norm_key: &str, limit: usize) -> Result<Vec<GraphNode>> {
    let mut stmt = conn.prepare("SELECT id,kind,ref_key,label,norm_key,confidence FROM graph_nodes WHERE norm_key LIKE ?1 ORDER BY confidence DESC,id LIMIT ?2").map_err(|e| Error::internal(format!("prepare graph node search: {e}")))?;
    let rows = stmt
        .query_map(params![format!("{}%", norm_key), limit as i64], |r| {
            Ok(GraphNode {
                id: r.get(0)?,
                kind: r.get(1)?,
                ref_key: r.get(2)?,
                label: r.get(3)?,
                norm_key: r.get(4)?,
                confidence: r.get(5)?,
            })
        })
        .map_err(|e| Error::internal(format!("graph node search: {e}")))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("read graph node search: {e}")))
}

/// Return typed edges incident to a bounded node set. The caller applies the
/// query-specific scoring; this keeps SQL traversal cheap and explainable.
pub fn incident_edges(
    conn: &Connection,
    node_ids: &[i64],
    relations: &[&str],
    limit: usize,
) -> Result<Vec<MemoryGraphEdge>> {
    if node_ids.is_empty() || relations.is_empty() {
        return Ok(Vec::new());
    }
    let nodes = std::iter::repeat("?")
        .take(node_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let rels = std::iter::repeat("?")
        .take(relations.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT id,from_node,to_node,relation,weight,confidence,src_observation_id,source,metadata_json FROM graph_edges WHERE (from_node IN ({nodes}) OR to_node IN ({nodes})) AND relation IN ({rels}) ORDER BY confidence DESC, weight DESC, id LIMIT ?");
    let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    for id in node_ids {
        values.push(Box::new(*id));
    }
    for id in node_ids {
        values.push(Box::new(*id));
    }
    for relation in relations {
        values.push(Box::new((*relation).to_string()));
    }
    values.push(Box::new(limit as i64));
    let refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare graph edges: {e}")))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(refs), |r| {
            Ok(MemoryGraphEdge {
                id: r.get(0)?,
                from_node: r.get(1)?,
                to_node: r.get(2)?,
                relation: r.get(3)?,
                weight: r.get(4)?,
                confidence: r.get(5)?,
                src_observation_id: r.get(6)?,
                source: r.get(7)?,
                metadata_json: r.get(8)?,
            })
        })
        .map_err(|e| Error::internal(format!("graph edges: {e}")))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| Error::internal(format!("read graph edges: {e}")))
}

pub fn node_by_id(conn: &Connection, id: i64) -> Result<Option<GraphNode>> {
    conn.query_row(
        "SELECT id,kind,ref_key,label,norm_key,confidence FROM graph_nodes WHERE id=?1",
        params![id],
        |r| {
            Ok(GraphNode {
                id: r.get(0)?,
                kind: r.get(1)?,
                ref_key: r.get(2)?,
                label: r.get(3)?,
                norm_key: r.get(4)?,
                confidence: r.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| Error::internal(format!("lookup graph node id: {e}")))
}

pub fn edge_by_id(conn: &Connection, id: i64) -> Result<Option<MemoryGraphEdge>> {
    conn.query_row(
        "SELECT id,from_node,to_node,relation,weight,confidence,src_observation_id,source,metadata_json FROM graph_edges WHERE id=?1",
        params![id],
        |r| Ok(MemoryGraphEdge { id:r.get(0)?, from_node:r.get(1)?, to_node:r.get(2)?, relation:r.get(3)?, weight:r.get(4)?, confidence:r.get(5)?, src_observation_id:r.get(6)?, source:r.get(7)?, metadata_json:r.get(8)? }),
    ).optional().map_err(|e| Error::internal(format!("lookup graph edge id: {e}")))
}

/// Bounded bidirectional walk over typed edges. This is deliberately a small
/// beam rather than an unbounded recursive CTE: it keeps latency predictable,
/// preserves the best provenance path, and is safe around hub entities.
pub fn bounded_walk(
    conn: &Connection,
    seeds: &[i64],
    relations: &[&str],
    hops: u8,
    degree_cap: usize,
    beam_width: usize,
) -> Result<Vec<GraphPath>> {
    if seeds.is_empty() || relations.is_empty() || hops == 0 {
        return Ok(Vec::new());
    }
    let mut frontier = seeds
        .iter()
        .copied()
        .map(|id| GraphPath {
            node_ids: vec![id],
            edge_ids: Vec::new(),
            score: 1.0,
            hops: 0,
        })
        .collect::<Vec<_>>();
    let mut best = frontier.clone();
    let mut seen = std::collections::HashMap::<i64, f64>::new();
    for &seed in seeds {
        seen.insert(seed, 1.0);
    }
    for _ in 0..hops {
        let ids = frontier
            .iter()
            .filter_map(|p| p.node_ids.last().copied())
            .collect::<Vec<_>>();
        let edges = incident_edges(
            conn,
            &ids,
            relations,
            degree_cap.saturating_mul(ids.len()).max(1),
        )?;
        let mut next = Vec::new();
        for path in frontier {
            let Some(current) = path.node_ids.last().copied() else {
                continue;
            };
            for edge in edges
                .iter()
                .filter(|e| e.from_node == current || e.to_node == current)
            {
                let neighbor = if edge.from_node == current {
                    edge.to_node
                } else {
                    edge.from_node
                };
                if path.node_ids.contains(&neighbor) {
                    continue;
                }
                let score =
                    path.score * edge.confidence.max(0.01) * edge.weight.max(0.01).sqrt() * 0.65;
                if score <= seen.get(&neighbor).copied().unwrap_or(0.0) * 0.85 {
                    continue;
                }
                seen.insert(neighbor, score);
                let mut node_ids = path.node_ids.clone();
                node_ids.push(neighbor);
                let mut edge_ids = path.edge_ids.clone();
                edge_ids.push(edge.id);
                next.push(GraphPath {
                    node_ids,
                    edge_ids,
                    score,
                    hops: path.hops + 1,
                });
            }
        }
        next.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.node_ids.cmp(&b.node_ids))
        });
        next.truncate(beam_width.max(1));
        best.extend(next.iter().cloned());
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    best.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.node_ids.cmp(&b.node_ids))
    });
    Ok(best)
}

/// Remove deterministic edges for an observation before re-indexing.
///
/// LLM-derived fact edges share the observation provenance but must survive a
/// later anchor/entity refresh; only the deterministic projection is rebuilt.
pub fn remove_observation_projection(conn: &Connection, observation_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM graph_edges WHERE src_observation_id=?1 AND source='deterministic'",
        params![observation_id],
    )
    .map_err(|e| Error::internal(format!("remove graph observation edges: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_write;
    use tempfile::TempDir;

    #[test]
    fn typed_walk_preserves_confidence_and_provenance() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("graph.db")).unwrap();
        let a = upsert_node(
            &conn,
            "entity",
            "entity:a",
            "A",
            "a",
            None,
            1.0,
            "deterministic",
        )
        .unwrap();
        let b = upsert_node(
            &conn,
            "fact",
            "fact:1",
            "A likes B",
            "a|likes|b",
            None,
            0.8,
            "llm",
        )
        .unwrap();
        let c = upsert_node(
            &conn,
            "observation",
            "observation:7",
            "source",
            "source",
            None,
            1.0,
            "deterministic",
        )
        .unwrap();
        insert_edge(
            &conn,
            a,
            b,
            "subject",
            1.0,
            0.9,
            Some("2025-01-01"),
            None,
            "llm",
            Some(7),
            Some("A"),
            None,
        )
        .unwrap();
        insert_edge(
            &conn,
            b,
            c,
            "derived_from",
            1.0,
            1.0,
            None,
            None,
            "llm",
            Some(7),
            None,
            None,
        )
        .unwrap();
        let paths = bounded_walk(&conn, &[a], &["subject", "derived_from"], 2, 32, 16).unwrap();
        assert!(paths.iter().any(|p| p.node_ids.last() == Some(&c)));
        let edge = incident_edges(&conn, &[a], &["subject"], 10).unwrap();
        assert_eq!(edge[0].src_observation_id, Some(7));
        assert_eq!(edge[0].source, "llm");
    }

    #[test]
    fn projection_edge_upsert_is_idempotent_for_same_source() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("graph.db")).unwrap();
        let a = upsert_node(
            &conn,
            "entity",
            "entity:a",
            "A",
            "a",
            None,
            1.0,
            "deterministic",
        )
        .unwrap();
        let b = upsert_node(
            &conn,
            "entity",
            "entity:b",
            "B",
            "b",
            None,
            1.0,
            "deterministic",
        )
        .unwrap();
        insert_edge(
            &conn,
            a,
            b,
            "mentions",
            1.0,
            1.0,
            None,
            None,
            "deterministic",
            Some(3),
            None,
            None,
        )
        .unwrap();
        insert_edge(
            &conn,
            a,
            b,
            "mentions",
            2.0,
            1.0,
            None,
            None,
            "deterministic",
            Some(3),
            None,
            None,
        )
        .unwrap();
        let weight: f64 = conn
            .query_row("SELECT weight FROM graph_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(weight, 3.0);
    }

    #[test]
    fn deterministic_reindex_preserves_llm_fact_edges() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("graph.db")).unwrap();
        let a = upsert_node(
            &conn,
            "entity",
            "entity:a",
            "A",
            "a",
            None,
            1.0,
            "deterministic",
        )
        .unwrap();
        let b = upsert_node(
            &conn,
            "fact",
            "fact:1",
            "A likes B",
            "a|likes|b",
            None,
            0.8,
            "llm",
        )
        .unwrap();
        insert_edge(
            &conn,
            a,
            b,
            "subject",
            1.0,
            0.9,
            None,
            None,
            "llm",
            Some(9),
            None,
            None,
        )
        .unwrap();
        insert_edge(
            &conn,
            a,
            b,
            "mentions",
            1.0,
            1.0,
            None,
            None,
            "deterministic",
            Some(9),
            None,
            None,
        )
        .unwrap();

        remove_observation_projection(&conn, 9).unwrap();

        let remaining: Vec<String> = conn
            .prepare("SELECT relation FROM graph_edges ORDER BY relation")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(remaining, vec!["subject"]);
    }
}
