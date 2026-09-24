use std::collections::HashMap;

use statefulmemory_embed::Embedder;
use statefulmemory_proto::{stateful_memory_visualization_server::StatefulMemoryVisualization, *};
use statefulmemory_storage::write::ObservationKey;
use statefulmemory_storage::{
    anchor, facts as facts_q, graph as graph_q, jobs as jobs_q, read as read_q, relations,
    stats as stats_q,
};
use tonic::{Request, Response, Status};

use crate::error_map::map;
use crate::service::{edge_to_proto, entity_to_proto, obs_to_proto, StatefulMemoryService};

fn auth_context<T>(request: &Request<T>) -> Option<crate::auth::AuthCtx> {
    request.extensions().get::<crate::auth::AuthCtx>().cloned()
}

#[allow(clippy::result_large_err)]
fn project_name(value: &str) -> Result<String, Status> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Status::invalid_argument("project_name is required"));
    }
    Ok(value.to_string())
}

fn bounded_limit(value: i32, default: i32, maximum: i32) -> usize {
    if value <= 0 {
        default as usize
    } else {
        (value.min(maximum)) as usize
    }
}

fn sql_error(context: &str, error: rusqlite::Error) -> Status {
    Status::internal(format!("{context}: {error}"))
}

fn observation_proto(observation: statefulmemory_storage::Observation) -> Observation {
    obs_to_proto(observation)
}

#[tonic::async_trait]
impl StatefulMemoryVisualization for StatefulMemoryService {
    async fn get_observation_detail(
        &self,
        request: Request<GetObservationDetailRequest>,
    ) -> Result<Response<GetObservationDetailResponse>, Status> {
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        if request.observation_id <= 0 {
            return Err(Status::invalid_argument("observation_id must be positive"));
        }
        let project = map(self.open_project(&project_name))?;
        let conn = map(project.open_read_conn())?;
        let observation = map(read_q::get(
            &conn,
            &ObservationKey::Id(request.observation_id),
        ))?;
        let anchors = map(anchor::anchors_for(&conn, request.observation_id))?
            .into_iter()
            .map(|item| VisualizationAnchor {
                path: item.path,
                symbol: item.symbol,
                line_start: item.line_start.map(|value| value as i32),
                line_end: item.line_end.map(|value| value as i32),
                anchor_commit: item.anchor_commit,
                content_digest: item.content_digest,
            })
            .collect();
        let facts = map(facts_q::facts_for_obs(&conn, request.observation_id))?
            .into_iter()
            .map(|item| Fact {
                id: item.id,
                obs_id: item.obs_id,
                subject: item.subject,
                predicate: item.predicate,
                object: item.object,
                temporal: item.temporal,
                salience: item.salience,
                superseded_by: item.superseded_by,
                extracted_by: item.extracted_by,
                extracted_at: item.extracted_at,
            })
            .collect();
        let relations = map(relations::get_relations_for_observation(
            &conn,
            request.observation_id,
        ))?
        .into_iter()
        .map(|item| ObservationRelation {
            id: item.id,
            source_id: item.source_id,
            target_id: item.target_id,
            relation_type: item.relation_type,
            confidence: item.confidence,
            created_at: item.created_at,
        })
        .collect();
        let entity_ids = conn
            .prepare(
                "SELECT DISTINCT entity_id FROM entity_mentions
                 WHERE observation_id = ?1 ORDER BY entity_id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([request.observation_id], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| sql_error("load observation entities", error))?;
        let entities = map(graph_q::entities_by_ids(&conn, &entity_ids))?
            .into_iter()
            .map(entity_to_proto)
            .collect();
        let edges = if entity_ids.is_empty() {
            Vec::new()
        } else {
            map(graph_q::edges_for_entities(
                &conn,
                &entity_ids,
                &["mentions", "fixes", "contradicts"],
                128,
            ))?
            .into_iter()
            .map(edge_to_proto)
            .collect()
        };
        let history = map(read_q::history_chain(&conn, request.observation_id))?
            .into_iter()
            .map(|entry| observation_proto(entry.observation))
            .collect();
        Ok(Response::new(GetObservationDetailResponse {
            observation: Some(observation_proto(observation)),
            anchors,
            facts,
            relations,
            entities,
            edges,
            history,
        }))
    }

    async fn get_health(
        &self,
        request: Request<GetHealthRequest>,
    ) -> Result<Response<GetHealthResponse>, Status> {
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        let project = map(self.open_project(&project_name))?;
        let conn = map(project.open_read_conn())?;
        let health = map(statefulmemory_storage::doctor::project_health_snapshot(
            &conn,
            &project_name,
        ))?;
        let projections = health.projections;
        let state = if projections.is_healthy() {
            "healthy"
        } else {
            "degraded"
        };
        Ok(Response::new(GetHealthResponse {
            project_name,
            state: state.into(),
            database_integrity: projections.database_integrity,
            total_observations: projections.total_observations,
            active_observations: projections.active_observations,
            soft_deleted_observations: projections.soft_deleted_observations,
            fts_rows: projections.fts_rows,
            fts_missing: projections.fts_missing,
            fts_stale: projections.fts_stale,
            embedding_rows: projections.embedding_rows,
            embedding_missing: projections.embedding_missing,
            orphan_embeddings: projections.orphan_embeddings,
            fact_rows: projections.fact_rows,
            anchor_rows: projections.anchor_rows,
            entities: projections.graph.total_entities,
            entity_mentions: projections.graph.total_mentions,
            entity_edges: projections.graph.total_edges,
            pending_jobs: projections.jobs.pending,
            running_jobs: projections.jobs.running,
            dead_jobs: projections.jobs.dead,
            graph_coverage: projections.graph.coverage_ratio,
        }))
    }

    async fn explain_retrieval(
        &self,
        request: Request<ExplainRetrievalRequest>,
    ) -> Result<Response<ExplainRetrievalResponse>, Status> {
        let started = std::time::Instant::now();
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        let query = request.query.trim().to_string();
        if query.is_empty() {
            return Err(Status::invalid_argument("query is required"));
        }
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        let project = map(self.open_project(&project_name))?;
        let conn = map(project.open_read_conn())?;
        let limit = bounded_limit(request.limit, 15, 50);
        let mode = request.mode.clone().unwrap_or_else(|| "hybrid".into());
        let bm25 = map(read_q::search(&conn, &query, None, None, 30))?;
        let bm25_ids = bm25.iter().map(|item| item.id as u64).collect::<Vec<_>>();
        let mut dense = Vec::new();
        if mode.eq_ignore_ascii_case("hybrid") {
            let embedder = self.state.query_embedder.read().clone();
            if let Some(embedder) = embedder {
                if let Ok(mut vectors) = embedder.embed(&[query.as_str()]) {
                    if let Some(vector) = vectors.pop() {
                        dense = map(read_q::search_dense(&conn, &vector, 30))?;
                    }
                }
            }
        }
        let dense_ids = dense.iter().map(|item| item.id as u64).collect::<Vec<_>>();
        let fused = statefulmemory_retrieval::rrf::rrf_fuse_scored(
            &[bm25_ids.clone(), dense_ids.clone()],
            60,
        );
        let bm25_ranks = ranks(&bm25_ids);
        let dense_ranks = ranks(&dense_ids);
        let mut candidates = Vec::new();
        for (index, (observation_id, score)) in fused.into_iter().enumerate() {
            let fused_rank = index as i32 + 1;
            let mut sources = Vec::new();
            if bm25_ranks.contains_key(&observation_id) {
                sources.push("bm25".into());
            }
            if dense_ranks.contains_key(&observation_id) {
                sources.push("dense".into());
            }
            candidates.push(VisualizationCandidate {
                observation_id: observation_id as i64,
                sources,
                bm25_rank: *bm25_ranks.get(&observation_id).unwrap_or(&0),
                dense_rank: *dense_ranks.get(&observation_id).unwrap_or(&0),
                fused_rank,
                final_rank: fused_rank,
                score,
            });
        }
        let mut results = Vec::new();
        for candidate in candidates.iter().take(limit) {
            if let Ok(observation) =
                read_q::get(&conn, &ObservationKey::Id(candidate.observation_id))
            {
                results.push(observation_proto(observation));
            }
        }
        let elapsed_us = started.elapsed().as_micros().min(i64::MAX as u128) as i64;
        Ok(Response::new(ExplainRetrievalResponse {
            query,
            mode,
            rerank: request.rerank.unwrap_or_default(),
            candidate_depth: candidates.len() as i32,
            rerank_timed_out: false,
            elapsed_us,
            candidates,
            results,
            tokens_used: Some(0),
        }))
    }

    async fn list_jobs(
        &self,
        request: Request<ListJobsRequest>,
    ) -> Result<Response<ListJobsResponse>, Status> {
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        let project = map(self.open_project(&project_name))?;
        let conn = map(project.open_read_conn())?;
        let status_filter = if request.status.trim().is_empty() {
            None
        } else {
            Some(
                jobs_q::JobStatus::parse(request.status.trim())
                    .ok_or_else(|| Status::invalid_argument("invalid job status"))?,
            )
        };
        let kind_filter = (!request.kind.trim().is_empty()).then(|| request.kind.clone());
        let filter = jobs_q::JobFilter {
            status: status_filter,
            kind: kind_filter,
            project_name: Some(project_name.clone()),
            limit: bounded_limit(request.limit, 50, 200),
            offset: 0,
        };
        let rows = map(jobs_q::list_filtered(&conn, &filter))?;
        let total = map(jobs_q::count_filtered(&conn, &filter))?;
        let counts = map(jobs_q::status_summary(&conn))?;
        Ok(Response::new(ListJobsResponse {
            jobs: rows
                .into_iter()
                .map(|item| VisualizationJob {
                    id: item.id,
                    kind: item.kind,
                    project_name: item.project_name,
                    observation_id: item.observation_id,
                    dedupe_key: item.dedupe_key,
                    status: item.status,
                    attempts: item.attempts,
                    max_attempts: item.max_attempts,
                    available_at: item.available_at,
                    locked_at: item.locked_at,
                    last_error: item.last_error,
                    created_at: item.created_at,
                    updated_at: item.updated_at,
                    payload: item.payload,
                })
                .collect(),
            total,
            pending: counts.pending,
            running: counts.running,
            completed: counts.completed,
            dead: counts.dead,
        }))
    }

    async fn get_graph_stats(
        &self,
        request: Request<GetGraphStatsRequest>,
    ) -> Result<Response<GetGraphStatsResponse>, Status> {
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        let project = map(self.open_project(&project_name))?;
        let conn = map(project.open_read_conn())?;
        let stats = map(graph_q::stats(&conn))?;
        let coverage = map(graph_q::coverage(&conn))?;
        Ok(Response::new(GetGraphStatsResponse {
            project_name,
            total_entities: stats.total_entities,
            total_mentions: stats.total_mentions,
            total_edges: stats.total_edges,
            covered_observations: coverage.covered_observations,
            coverage_ratio: coverage.coverage_ratio,
            entities_by_kind: stats
                .entities_by_kind
                .into_iter()
                .map(|(name, value)| VisualizationCount { name, value })
                .collect(),
            edges_by_relation: stats
                .edges_by_relation
                .into_iter()
                .map(|(name, value)| VisualizationCount { name, value })
                .collect(),
        }))
    }

    async fn get_sync_state(
        &self,
        request: Request<GetSyncStateRequest>,
    ) -> Result<Response<GetSyncStateResponse>, Status> {
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        let project = map(self.open_project(&project_name))?;
        let status = crate::sync_status::compute(
            &self.state,
            &project,
            &SyncStatusRequest {
                project_name: Some(project_name.clone()),
            },
        )
        .await?;
        Ok(Response::new(GetSyncStateResponse {
            project_name,
            last_export_at: status.last_export_at,
            last_import_at: status.last_import_at,
            unseen_chunk_count: status.unseen_chunk_count,
            last_error: status.last_error,
            total_exported_chunks: status.total_exported_chunks,
            total_imported_chunks: status.total_imported_chunks,
        }))
    }

    async fn get_project_summary(
        &self,
        request: Request<GetProjectSummaryRequest>,
    ) -> Result<Response<GetProjectSummaryResponse>, Status> {
        let auth = auth_context(&request);
        let request = request.into_inner();
        let project_name = project_name(&request.project_name)?;
        self.authorize_project(auth.as_ref(), &project_name, false)?;
        let project = map(self.open_project(&project_name))?;
        let conn = map(project.open_read_conn())?;
        let summary = map(stats_q::project_summary(&conn, &project_name))?;
        Ok(Response::new(GetProjectSummaryResponse {
            summary: Some(summary_proto(summary)),
        }))
    }

    async fn list_project_summaries(
        &self,
        request: Request<ListProjectSummariesRequest>,
    ) -> Result<Response<ListProjectSummariesResponse>, Status> {
        if self.state.auth_required {
            self.authorize_admin(&request)?;
        }
        let projects = map(statefulmemory_storage::ProjectRegistry::list_known_on_disk())?;
        let mut summaries = Vec::new();
        for (name, _) in projects {
            let project = match self.open_project(&name) {
                Ok(project) => project,
                Err(_) => continue,
            };
            let conn = match project.open_read_conn() {
                Ok(conn) => conn,
                Err(_) => continue,
            };
            if let Ok(summary) = stats_q::project_summary(&conn, &name) {
                summaries.push(summary_proto(summary));
            }
        }
        Ok(Response::new(ListProjectSummariesResponse {
            projects: summaries,
        }))
    }
}

fn ranks(ids: &[u64]) -> HashMap<u64, i32> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| (*id, index as i32 + 1))
        .collect()
}

fn summary_proto(summary: stats_q::ProjectSummary) -> VisualizationProjectSummary {
    VisualizationProjectSummary {
        project: summary.project,
        observations: summary.observations,
        soft_deleted_observations: summary.soft_deleted_observations,
        sessions: summary.sessions,
        prompts: summary.prompts,
        facts: summary.facts,
        active_facts: summary.active_facts,
        embedded_observations: summary.embedded_observations,
        anchor_rows: summary.anchor_rows,
        anchored_observations: summary.anchored_observations,
        entities: summary.entities,
        entity_mentions: summary.entity_mentions,
        entity_edges: summary.entity_edges,
        jobs: summary.jobs,
        pending_jobs: summary.pending_jobs,
        running_jobs: summary.running_jobs,
        dead_jobs: summary.dead_jobs,
        latest_observation_at: summary.latest_observation_at,
        latest_updated_at: summary.latest_updated_at,
    }
}
