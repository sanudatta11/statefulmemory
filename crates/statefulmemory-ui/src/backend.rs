use async_trait::async_trait;
use serde_json::{json, Value};
use statefulmemory_client::StatefulMemoryClient;
use statefulmemory_proto as p;
use statefulmemory_proto::stateful_memory_visualization_client::StatefulMemoryVisualizationClient;
use thiserror::Error;
use tonic::{transport::Channel, Code, Status};

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct BackendError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl BackendError {
    pub fn new(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "not_found", message)
    }
}

#[async_trait]
pub trait UiBackend: Send + Sync {
    async fn search(
        &self,
        project_name: &str,
        query: &str,
        limit: i32,
    ) -> Result<Value, BackendError>;

    async fn graph(
        &self,
        project_name: &str,
        entity: &str,
        hops: u32,
    ) -> Result<Value, BackendError>;

    async fn stats(&self, project_name: &str) -> Result<Value, BackendError>;

    async fn decide(
        &self,
        project_name: &str,
        question: &str,
        limit: i32,
    ) -> Result<Value, BackendError>;

    async fn list_memories(&self, _project_name: &str, _limit: i32) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "memory list is unavailable",
        ))
    }

    async fn project_summary(&self, _project_name: &str) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "project summary is unavailable",
        ))
    }

    async fn health(&self, _project_name: &str) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "health data is unavailable",
        ))
    }

    async fn jobs(
        &self,
        _project_name: &str,
        _status: &str,
        _kind: &str,
        _limit: i32,
    ) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "job data is unavailable",
        ))
    }

    async fn sync_state(&self, _project_name: &str) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "sync data is unavailable",
        ))
    }

    async fn graph_stats(&self, _project_name: &str) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "graph stats are unavailable",
        ))
    }

    async fn explain(
        &self,
        _project_name: &str,
        _query: &str,
        _limit: i32,
        _mode: &str,
        _rerank: &str,
    ) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "retrieval explanation is unavailable",
        ))
    }

    async fn observation_detail(
        &self,
        _project_name: &str,
        _observation_id: i64,
    ) -> Result<Value, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "observation detail is unavailable",
        ))
    }

    async fn project_summaries(&self) -> Result<Vec<Value>, BackendError> {
        Err(BackendError::new(
            501,
            "not_implemented",
            "project list is unavailable",
        ))
    }
}

#[derive(Clone)]
pub struct GrpcBackend {
    client: StatefulMemoryClient<Channel>,
    visualization: StatefulMemoryVisualizationClient<Channel>,
}

impl GrpcBackend {
    pub fn new(channel: Channel) -> Self {
        Self {
            client: StatefulMemoryClient::new(channel.clone()),
            visualization: StatefulMemoryVisualizationClient::new(channel),
        }
    }
}

#[async_trait]
impl UiBackend for GrpcBackend {
    async fn search(
        &self,
        project_name: &str,
        query: &str,
        limit: i32,
    ) -> Result<Value, BackendError> {
        let response = self
            .client
            .clone()
            .search_observations(p::SearchObservationsRequest {
                project_name: project_name.to_string(),
                query: query.to_string(),
                r#type: None,
                scope: None,
                all_projects: false,
                limit,
                mode: Some("hybrid".to_string()),
                rerank: None,
                max_tokens: Some(4096),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        let hits = response
            .observations
            .iter()
            .take(50)
            .map(|observation| {
                json!({
                    "id": observation.id,
                    "type": observation.r#type,
                    "title": observation.title,
                    "content": preview(&observation.content, 160),
                    "created_at": observation.created_at,
                    "verify_state": observation.verify_state.as_deref().unwrap_or(""),
                    "code_anchor": observation.code_anchor.as_deref().unwrap_or(""),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "hits": hits,
            "tokens_used": response.tokens_used,
            "warning": response.warning,
        }))
    }

    async fn graph(
        &self,
        project_name: &str,
        entity: &str,
        hops: u32,
    ) -> Result<Value, BackendError> {
        let normalized = normalize_name(entity);
        let list = self
            .client
            .clone()
            .list_entities(p::ListEntitiesRequest {
                project_name: project_name.to_string(),
                norm_prefix: normalized.clone(),
                kind_filter: String::new(),
                limit: 20,
            })
            .await
            .map_err(map_status)?
            .into_inner();
        let seed = pick_entity(&list.entities, &normalized, entity)
            .ok_or_else(|| BackendError::not_found("entity not found"))?;
        let response = self
            .client
            .clone()
            .graph_query(p::GraphQueryRequest {
                project_name: project_name.to_string(),
                entity_id: seed.id,
                hops,
                edge_types: Vec::new(),
                limit: 64,
                relation_filter: String::new(),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        let seed_name = response
            .entities
            .iter()
            .find(|candidate| candidate.id == seed.id)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_else(|| seed.name.clone());
        let entities = response
            .entities
            .iter()
            .map(|candidate| {
                json!({
                    "id": candidate.id,
                    "kind": candidate.kind,
                    "name": candidate.name,
                    "norm_name": candidate.norm_name,
                })
            })
            .collect::<Vec<_>>();
        let edges = response
            .edges
            .iter()
            .map(|edge| {
                json!({
                    "source": edge.from_id,
                    "target": edge.to_id,
                    "relation": edge.relation,
                    "weight": edge.weight,
                    "from_id": edge.from_id,
                    "to_id": edge.to_id,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "seed": seed_name,
            "seed_id": seed.id,
            "entities": entities,
            "edges": edges,
        }))
    }

    async fn stats(&self, project_name: &str) -> Result<Value, BackendError> {
        let response = self
            .client
            .clone()
            .dream_scan(p::DreamScanRequest {
                project_name: project_name.to_string(),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        let summary = self.project_summary(project_name).await?;
        let health = self.health(project_name).await?;
        Ok(json!({
            "observations_scanned": response.observations_scanned,
            "consolidation_proposals": response.proposals.len(),
            "summary": summary,
            "health": health,
        }))
    }

    async fn decide(
        &self,
        project_name: &str,
        question: &str,
        limit: i32,
    ) -> Result<Value, BackendError> {
        let response = self
            .client
            .clone()
            .decide(p::DecideRequest {
                project_name: project_name.to_string(),
                question: question.to_string(),
                limit,
                mode: Some("hybrid".to_string()),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(json!({
            "answer": response.recommendation,
            "reasoning": response.rationale,
            "confidence": response.confidence,
            "hits": response.evidence.len(),
            "evidence": response.evidence.iter().map(|evidence| json!({
                "observation_id": evidence.observation_id,
                "title": evidence.title,
                "content": preview(&evidence.content, 240),
                "role": evidence.role,
            })).collect::<Vec<_>>(),
            "conflicts": response.conflicts.iter().map(|conflict| json!({
                "a_id": conflict.a_id,
                "b_id": conflict.b_id,
                "relation": conflict.relation,
                "status": conflict.status,
            })).collect::<Vec<_>>()
        }))
    }

    async fn list_memories(&self, project_name: &str, limit: i32) -> Result<Value, BackendError> {
        let response = self
            .client
            .clone()
            .list_observations(p::ListObservationsRequest {
                project_name: project_name.to_string(),
                r#type: None,
                scope: None,
                created_by: None,
                due_for_review: false,
                limit,
                cursor: None,
                session_id: None,
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(json!({
            "memories": response
                .observations
                .iter()
                .map(|item| observation_value(item, project_name))
                .collect::<Vec<_>>(),
        }))
    }

    async fn project_summary(&self, project_name: &str) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .get_project_summary(p::GetProjectSummaryRequest {
                project_name: project_name.to_string(),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        response
            .summary
            .map(|summary| summary_value(&summary))
            .ok_or_else(|| BackendError::new(502, "daemon_error", "project summary missing"))
    }

    async fn health(&self, project_name: &str) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .get_health(p::GetHealthRequest {
                project_name: project_name.to_string(),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(health_value(response))
    }

    async fn jobs(
        &self,
        project_name: &str,
        status: &str,
        kind: &str,
        limit: i32,
    ) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .list_jobs(p::ListJobsRequest {
                project_name: project_name.to_string(),
                status: status.to_string(),
                kind: kind.to_string(),
                limit,
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(json!({
            "jobs": response.jobs.iter().map(job_value).collect::<Vec<_>>(),
            "total": response.total,
            "pending": response.pending,
            "running": response.running,
            "completed": response.completed,
            "dead": response.dead,
        }))
    }

    async fn sync_state(&self, project_name: &str) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .get_sync_state(p::GetSyncStateRequest {
                project_name: project_name.to_string(),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(json!({
            "project": response.project_name,
            "last_export_at": response.last_export_at,
            "last_import_at": response.last_import_at,
            "unseen_chunk_count": response.unseen_chunk_count,
            "last_error": response.last_error,
            "total_exported_chunks": response.total_exported_chunks,
            "total_imported_chunks": response.total_imported_chunks,
        }))
    }

    async fn graph_stats(&self, project_name: &str) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .get_graph_stats(p::GetGraphStatsRequest {
                project_name: project_name.to_string(),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(json!({
            "project": response.project_name,
            "total_entities": response.total_entities,
            "total_mentions": response.total_mentions,
            "total_edges": response.total_edges,
            "covered_observations": response.covered_observations,
            "coverage_ratio": response.coverage_ratio,
            "entities_by_kind": response.entities_by_kind.iter().map(count_value).collect::<Vec<_>>(),
            "edges_by_relation": response.edges_by_relation.iter().map(count_value).collect::<Vec<_>>(),
        }))
    }

    async fn explain(
        &self,
        project_name: &str,
        query: &str,
        limit: i32,
        mode: &str,
        rerank: &str,
    ) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .explain_retrieval(p::ExplainRetrievalRequest {
                project_name: project_name.to_string(),
                query: query.to_string(),
                limit,
                mode: Some(mode.to_string()),
                rerank: (!rerank.trim().is_empty()).then(|| rerank.to_string()),
            })
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(json!({
            "query": response.query,
            "mode": response.mode,
            "rerank": response.rerank,
            "candidate_depth": response.candidate_depth,
            "rerank_timed_out": response.rerank_timed_out,
            "elapsed_us": response.elapsed_us,
            "candidates": response.candidates.iter().map(candidate_value).collect::<Vec<_>>(),
            "results": response.results.iter().map(|item| observation_value(item, project_name)).collect::<Vec<_>>(),
            "tokens_used": response.tokens_used,
        }))
    }

    async fn observation_detail(
        &self,
        project_name: &str,
        observation_id: i64,
    ) -> Result<Value, BackendError> {
        let response = self
            .visualization
            .clone()
            .get_observation_detail(p::GetObservationDetailRequest {
                project_name: project_name.to_string(),
                observation_id,
            })
            .await
            .map_err(map_status)?
            .into_inner();
        let observation = response
            .observation
            .ok_or_else(|| BackendError::not_found("observation not found"))?;
        Ok(json!({
            "observation": observation_value(&observation, project_name),
            "anchors": response.anchors.iter().map(anchor_value).collect::<Vec<_>>(),
            "facts": response.facts.iter().map(fact_value).collect::<Vec<_>>(),
            "relations": response.relations.iter().map(relation_value).collect::<Vec<_>>(),
            "entities": response.entities.iter().map(entity_value).collect::<Vec<_>>(),
            "edges": response.edges.iter().map(edge_value).collect::<Vec<_>>(),
            "history": response.history.iter().map(|item| observation_value(item, project_name)).collect::<Vec<_>>(),
        }))
    }

    async fn project_summaries(&self) -> Result<Vec<Value>, BackendError> {
        let response = self
            .visualization
            .clone()
            .list_project_summaries(p::ListProjectSummariesRequest {})
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response.projects.iter().map(summary_value).collect())
    }
}

fn summary_value(summary: &p::VisualizationProjectSummary) -> Value {
    json!({
        "project": summary.project,
        "observations": summary.observations,
        "soft_deleted_observations": summary.soft_deleted_observations,
        "sessions": summary.sessions,
        "prompts": summary.prompts,
        "facts": summary.facts,
        "active_facts": summary.active_facts,
        "embedded_observations": summary.embedded_observations,
        "anchor_rows": summary.anchor_rows,
        "anchored_observations": summary.anchored_observations,
        "entities": summary.entities,
        "entity_mentions": summary.entity_mentions,
        "entity_edges": summary.entity_edges,
        "jobs": summary.jobs,
        "pending_jobs": summary.pending_jobs,
        "running_jobs": summary.running_jobs,
        "dead_jobs": summary.dead_jobs,
        "latest_observation_at": summary.latest_observation_at,
        "latest_updated_at": summary.latest_updated_at,
    })
}

fn health_value(response: p::GetHealthResponse) -> Value {
    json!({
        "project": response.project_name,
        "state": response.state,
        "database_integrity": response.database_integrity,
        "total_observations": response.total_observations,
        "active_observations": response.active_observations,
        "soft_deleted_observations": response.soft_deleted_observations,
        "fts_rows": response.fts_rows,
        "fts_missing": response.fts_missing,
        "fts_stale": response.fts_stale,
        "embedding_rows": response.embedding_rows,
        "embedding_missing": response.embedding_missing,
        "orphan_embeddings": response.orphan_embeddings,
        "fact_rows": response.fact_rows,
        "anchor_rows": response.anchor_rows,
        "entities": response.entities,
        "entity_mentions": response.entity_mentions,
        "entity_edges": response.entity_edges,
        "pending_jobs": response.pending_jobs,
        "running_jobs": response.running_jobs,
        "dead_jobs": response.dead_jobs,
        "graph_coverage": response.graph_coverage,
    })
}

fn job_value(job: &p::VisualizationJob) -> Value {
    json!({
        "id": job.id,
        "kind": job.kind,
        "project_name": job.project_name,
        "observation_id": job.observation_id,
        "dedupe_key": job.dedupe_key,
        "status": job.status,
        "attempts": job.attempts,
        "max_attempts": job.max_attempts,
        "available_at": job.available_at,
        "locked_at": job.locked_at,
        "last_error": job.last_error,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "payload": job.payload,
    })
}

fn count_value(count: &p::VisualizationCount) -> Value {
    json!({ "name": count.name, "value": count.value })
}

fn candidate_value(candidate: &p::VisualizationCandidate) -> Value {
    json!({
        "observation_id": candidate.observation_id,
        "sources": candidate.sources,
        "bm25_rank": candidate.bm25_rank,
        "dense_rank": candidate.dense_rank,
        "fused_rank": candidate.fused_rank,
        "final_rank": candidate.final_rank,
        "score": candidate.score,
    })
}

fn observation_value(observation: &p::Observation, project_name: &str) -> Value {
    json!({
        "id": observation.id,
        "project_name": observation.project_name.as_deref().unwrap_or(project_name),
        "sync_id": observation.sync_id,
        "session_id": observation.session_id,
        "type": observation.r#type,
        "title": observation.title,
        "content": observation.content,
        "tool_name": observation.tool_name,
        "scope": observation.scope,
        "created_by": observation.created_by,
        "topic_key": observation.topic_key,
        "created_at": observation.created_at,
        "updated_at": observation.updated_at,
        "deleted_at": observation.deleted_at,
        "review_after": observation.review_after,
        "code_anchor": observation.code_anchor,
        "superseded_count": observation.superseded_count,
        "verify_state": observation.verify_state,
    })
}

fn entity_value(entity: &p::GraphEntity) -> Value {
    json!({
        "id": entity.id,
        "kind": entity.kind,
        "name": entity.name,
        "norm_name": entity.norm_name,
    })
}

fn edge_value(edge: &p::GraphEdge) -> Value {
    json!({
        "source": edge.from_id,
        "target": edge.to_id,
        "from_id": edge.from_id,
        "to_id": edge.to_id,
        "relation": edge.relation,
        "weight": edge.weight,
        "first_seen_epoch": edge.first_seen_epoch,
        "last_seen_epoch": edge.last_seen_epoch,
        "src_observation_id": edge.src_observation_id,
    })
}

fn anchor_value(anchor: &p::VisualizationAnchor) -> Value {
    json!({
        "path": anchor.path,
        "symbol": anchor.symbol,
        "line_start": anchor.line_start,
        "line_end": anchor.line_end,
        "anchor_commit": anchor.anchor_commit,
        "content_digest": anchor.content_digest,
    })
}

fn fact_value(fact: &p::Fact) -> Value {
    json!({
        "id": fact.id,
        "obs_id": fact.obs_id,
        "subject": fact.subject,
        "predicate": fact.predicate,
        "object": fact.object,
        "temporal": fact.temporal,
        "salience": fact.salience,
        "superseded_by": fact.superseded_by,
        "extracted_by": fact.extracted_by,
        "extracted_at": fact.extracted_at,
    })
}

fn relation_value(relation: &p::ObservationRelation) -> Value {
    json!({
        "id": relation.id,
        "source_id": relation.source_id,
        "target_id": relation.target_id,
        "relation_type": relation.relation_type,
        "confidence": relation.confidence,
        "created_at": relation.created_at,
    })
}

fn map_status(status: Status) -> BackendError {
    let (http_status, code) = match status.code() {
        Code::InvalidArgument => (400, "invalid_request"),
        Code::NotFound => (404, "not_found"),
        Code::Unauthenticated => (401, "unauthorized"),
        Code::PermissionDenied => (403, "forbidden"),
        Code::Unimplemented => (501, "not_implemented"),
        Code::Unavailable => (503, "daemon_unavailable"),
        Code::DeadlineExceeded => (504, "daemon_timeout"),
        _ => (502, "daemon_error"),
    };
    BackendError::new(http_status, code, status.message())
}

fn preview(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn normalize_name(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn pick_entity<'a>(
    entities: &'a [p::GraphEntity],
    normalized: &str,
    original: &str,
) -> Option<&'a p::GraphEntity> {
    entities
        .iter()
        .find(|entity| entity.norm_name == normalized)
        .or_else(|| {
            entities
                .iter()
                .find(|entity| entity.name.eq_ignore_ascii_case(original))
        })
        .or_else(|| {
            entities
                .iter()
                .filter(|entity| entity.norm_name.starts_with(normalized))
                .min_by_key(|entity| (entity.norm_name.len(), entity.name.len()))
        })
}

#[cfg(test)]
mod tests {
    use super::normalize_name;

    #[test]
    fn normalizes_graph_names_without_case_or_punctuation() {
        assert_eq!(normalize_name("Graph-Query::v2"), "graphqueryv2");
    }
}
