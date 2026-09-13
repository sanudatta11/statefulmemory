#![allow(clippy::result_large_err)]

//! `memlayer graph query|rebuild|stats` — entity graph CLI (spec: graph-briefing).
//!
//! Query resolves a seed entity over the daemon's additive graph RPCs.
//! Stats / Rebuild open the project SQLite file directly (doctor-style)
//! so they work with the daemon down.

use std::collections::HashMap;
use std::process::ExitCode;

use memlayer_proto as p;
use memlayer_storage::graph;

use crate::cli::{GraphQueryArgs, GraphVerb};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::Formatter;

/// Lowercase and drop non-alphanumerics. Mirrors the daemon-side entity
/// normalization so prefix matching lines up with `norm_name` rows.
pub fn normalize_name(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

const DEFAULT_HOPS: u8 = 2;
const QUERY_LIMIT: u32 = 64;
const LIST_LIMIT: u32 = 100;

pub async fn dispatch(
    client: Option<&mut Client>,
    project_name: &str,
    fmt: Formatter,
    verb: GraphVerb,
) -> ExitCode {
    let result = match verb {
        GraphVerb::Query(a) => match client {
            Some(c) => query(c, project_name, fmt, a).await,
            None => Err(VerbErr::Usage(
                "error: graph query requires the daemon".into(),
            )),
        },
        GraphVerb::Stats => stats(project_name, fmt),
        GraphVerb::Rebuild(a) => rebuild(project_name, a.quiet),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(VerbErr::Usage(msg)) => {
            eprintln!("{msg}");
            ExitCode::from(exit::USAGE)
        }
        Err(VerbErr::Local(msg)) => {
            eprintln!("memlayer: {msg}");
            ExitCode::from(exit::GENERAL)
        }
        Err(VerbErr::Status(s)) => {
            eprintln!("memlayer: {}", s.message());
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

enum VerbErr {
    Usage(String),
    Local(String),
    Status(tonic::Status),
}

/// Exact normalized match first; otherwise the shortest prefix match.
pub fn pick_entity_id(entities: &[p::GraphEntity], input: &str) -> Option<i64> {
    let norm = normalize_name(input);
    if let Some(e) = entities.iter().find(|e| e.norm_name == norm) {
        return Some(e.id);
    }
    let exact_name = input.to_lowercase();
    if let Some(e) = entities
        .iter()
        .find(|e| e.name.to_lowercase() == exact_name)
    {
        return Some(e.id);
    }
    entities
        .iter()
        .filter(|e| e.norm_name.starts_with(&norm))
        .min_by_key(|e| (e.norm_name.len(), e.name.len()))
        .map(|e| e.id)
}

async fn query(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: GraphQueryArgs,
) -> Result<(), VerbErr> {
    let list = client
        .list_entities(p::ListEntitiesRequest {
            project_name: project_name.to_string(),
            norm_prefix: normalize_name(&a.entity),
            kind_filter: String::new(),
            limit: LIST_LIMIT,
        })
        .await
        .map_err(VerbErr::Status)?
        .into_inner();

    let entity_id = pick_entity_id(&list.entities, &a.entity).ok_or_else(|| {
        VerbErr::Usage(format!(
            "error: entity not found: \"{}\" (run `memlayer graph rebuild` first)",
            a.entity
        ))
    })?;

    let resp = client
        .graph_query(p::GraphQueryRequest {
            project_name: project_name.to_string(),
            entity_id,
            hops: u32::from(a.hops.min(DEFAULT_HOPS)),
            edge_types: Vec::new(),
            limit: QUERY_LIMIT,
            relation_filter: a.relation.clone().unwrap_or_default(),
        })
        .await
        .map_err(VerbErr::Status)?
        .into_inner();

    let seed = resp
        .entities
        .iter()
        .find(|e| e.id == entity_id)
        .cloned()
        .unwrap_or(p::GraphEntity {
            id: entity_id,
            kind: String::new(),
            name: a.entity.clone(),
            norm_name: normalize_name(&a.entity),
        });

    if a.json {
        let v = graph_json(&seed, &resp);
        println!("{}", serde_json::to_string(&v).unwrap_or_default());
        return Ok(());
    }
    match fmt {
        Formatter::Text => print_text(&seed, &resp),
        _ => {
            let v = graph_json(&seed, &resp);
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
    }
    Ok(())
}

fn graph_json(seed: &p::GraphEntity, resp: &p::GraphQueryResponse) -> serde_json::Value {
    serde_json::json!({
        "seed": {
            "id": seed.id,
            "kind": seed.kind,
            "name": seed.name,
            "norm_name": seed.norm_name,
        },
        "entities": resp
            .entities
            .iter()
            .map(|e| serde_json::json!({
                "id": e.id,
                "kind": e.kind,
                "name": e.name,
                "norm_name": e.norm_name,
            }))
            .collect::<Vec<_>>(),
        "edges": resp
            .edges
            .iter()
            .map(|e| serde_json::json!({
                "from_id": e.from_id,
                "to_id": e.to_id,
                "relation": e.relation,
                "weight": e.weight,
                "src_observation_id": e.src_observation_id,
            }))
            .collect::<Vec<_>>(),
    })
}

fn print_text(seed: &p::GraphEntity, resp: &p::GraphQueryResponse) {
    println!(
        "graph around {} [{}] ({} edges)",
        seed.name,
        seed.kind,
        resp.edges.len()
    );
    let mut names: HashMap<i64, String> = HashMap::new();
    for e in &resp.entities {
        names.insert(e.id, e.name.clone());
    }
    let missing: Vec<i64> = resp
        .edges
        .iter()
        .flat_map(|e| [e.from_id, e.to_id])
        .filter(|id| !names.contains_key(id))
        .collect();
    for id in missing {
        names.entry(id).or_insert_with(|| id.to_string());
    }
    for e in &resp.edges {
        let from = names.get(&e.from_id).cloned().unwrap_or_default();
        let to = names.get(&e.to_id).cloned().unwrap_or_default();
        println!("  {from} --{}({:.2})--> {to}", e.relation, e.weight);
    }
}

fn project_db_path(project_name: &str) -> std::path::PathBuf {
    memlayer_core::paths::project_db_path(project_name)
}

fn stats(project_name: &str, fmt: Formatter) -> Result<(), VerbErr> {
    let db_path = project_db_path(project_name);
    if !db_path.exists() {
        return Err(VerbErr::Local(format!(
            "project database not found at {}",
            db_path.display()
        )));
    }
    let conn =
        memlayer_storage::db::open_read(&db_path).map_err(|e| VerbErr::Local(e.to_string()))?;
    let s = graph::stats(&conn).map_err(|e| VerbErr::Local(e.to_string()))?;
    match fmt {
        Formatter::Text => print!("{s}"),
        _ => {
            let v = serde_json::json!({
                "total_entities": s.total_entities,
                "entities_by_kind": s.entities_by_kind,
                "total_mentions": s.total_mentions,
                "total_edges": s.total_edges,
                "edges_by_relation": s.edges_by_relation,
            });
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
    }
    Ok(())
}

fn rebuild(project_name: &str, quiet: bool) -> Result<(), VerbErr> {
    let db_path = project_db_path(project_name);
    if !db_path.exists() {
        return Err(VerbErr::Local(format!(
            "project database not found at {}",
            db_path.display()
        )));
    }
    let conn =
        memlayer_storage::db::open_write(&db_path).map_err(|e| VerbErr::Local(e.to_string()))?;
    let created = graph::backfill_from_anchors(&conn).map_err(|e| VerbErr::Local(e.to_string()))?;
    println!("graph rebuild: {created} new entities from anchors");
    if !graph_enabled_in_config(project_name) && !quiet {
        eprintln!(
            "note: graph disabled in config; enable with `memlayer config set graph.enabled true`"
        );
    }
    Ok(())
}

/// Best-effort peek at the resolved `graph.enabled` flag by reading the
/// global and per-project config TOMLs directly (env > project > global).
fn graph_enabled_in_config(project_name: &str) -> bool {
    let mut enabled = false;
    for path in [
        memlayer_core::config::memlayer_global_config_path(),
        memlayer_core::config::memlayer_project_config_path(project_name),
    ] {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(toml) = raw.parse::<toml::Table>() {
                enabled = toml
                    .get("graph")
                    .and_then(|g| g.get("enabled"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(enabled);
            }
        }
    }
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: i64, name: &str, norm: &str) -> p::GraphEntity {
        p::GraphEntity {
            id,
            kind: "concept".into(),
            name: name.into(),
            norm_name: norm.into(),
        }
    }

    #[test]
    fn pick_exact_norm_match_beats_prefix() {
        let ents = [e(1, "auth", "auth"), e(2, "auth token", "authtoken")];
        assert_eq!(pick_entity_id(&ents, "auth"), Some(1));
        assert_eq!(pick_entity_id(&ents, "Auth Token"), Some(2));
    }

    #[test]
    fn pick_prefix_shortest_name_wins() {
        let ents = [
            e(7, "auth token refresh", "authtokenrefresh"),
            e(3, "auth token", "authtoken"),
        ];
        assert_eq!(pick_entity_id(&ents, "auth tok"), Some(3));
    }

    #[test]
    fn pick_normalizes_input() {
        let ents = [e(5, "WriteThread", "writethread")];
        assert_eq!(pick_entity_id(&ents, "write_thread"), Some(5));
        assert_eq!(pick_entity_id(&ents, "WRITE-THREAD"), Some(5));
    }

    #[test]
    fn pick_miss_returns_none() {
        let ents = [e(1, "auth", "auth")];
        assert_eq!(pick_entity_id(&ents, "vector"), None);
        assert_eq!(pick_entity_id(&[], "auth"), None);
    }
}
