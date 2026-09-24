import type {
  DashboardApi,
  DashboardStats,
  GraphResponse,
  GraphStats,
  HealthSnapshot,
  JobRecord,
  MemoryListResponse,
  MemoryRecord,
  ObservationDetail,
  ProjectOverview,
  ProjectSummary,
  RequestOptions,
  RetrievalCandidate,
  RetrievalExplanation,
  SearchResponse,
  SyncState,
} from './types'

interface HttpApiErrorPayload {
  error?: unknown
  message?: unknown
  detail?: unknown
}

type MemoryPayload = Record<string, unknown>

interface MemoryListPayload {
  memories?: MemoryPayload[]
  observations?: MemoryPayload[]
  nextCursor?: string
  next_cursor?: string
}

interface SearchPayload {
  memories?: MemoryPayload[]
  hits?: MemoryPayload[]
  warning?: string
  tokensUsed?: number
  tokens_used?: number
}

interface StatsPayload {
  observations_scanned?: unknown
  consolidation_proposals?: unknown
  summary?: Record<string, unknown>
  health?: Record<string, unknown>
}

export class DashboardApiError extends Error {
  readonly status: number
  readonly code: string
  readonly retryable: boolean

  constructor(message: string, status = 0, code = 'request_failed', cause?: unknown) {
    super(message, { cause })
    this.name = 'DashboardApiError'
    this.status = status
    this.code = code
    this.retryable = status === 0 || status === 408 || status === 429 || status >= 500
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function errorMessage(payload: unknown, fallback: string): string {
  if (typeof payload === 'string' && payload.trim()) {
    return payload
  }
  if (isRecord(payload)) {
    const errorPayload = payload as HttpApiErrorPayload
    for (const value of [errorPayload.error, errorPayload.message, errorPayload.detail]) {
      if (typeof value === 'string' && value.trim()) {
        return value
      }
    }
  }
  return fallback
}

function abortError(signal: AbortSignal): DOMException {
  return new DOMException(signal.reason ? String(signal.reason) : 'Request cancelled', 'AbortError')
}

function finiteNumber(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function text(value: unknown, fallback = ''): string {
  if (typeof value === 'string') return value
  if (typeof value === 'number') return String(value)
  return fallback
}

function memoryType(value: unknown): MemoryRecord['type'] {
  const candidate = text(value)
  return ['decision', 'pattern', 'fix', 'preference', 'context', 'fact'].includes(candidate)
    ? (candidate as MemoryRecord['type'])
    : 'context'
}

function memoryScope(value: unknown): MemoryRecord['scope'] {
  const candidate = text(value)
  return ['project', 'personal', 'team'].includes(candidate)
    ? (candidate as MemoryRecord['scope'])
    : 'project'
}

function verifyState(value: unknown): MemoryRecord['verifyState'] {
  const candidate = text(value, 'unanchored')
  return ['verified', 'unanchored', 'stale', 'invalidated', 'unprovable'].includes(candidate)
    ? (candidate as MemoryRecord['verifyState'])
    : 'unanchored'
}

function normalizeProjectSummary(payload: unknown): ProjectSummary {
  const record = isRecord(payload) ? payload : {}
  const name = text(record.name ?? record.project, 'unknown')
  const lastActiveAt = text(record.lastActiveAt ?? record.last_active_at ?? record.latest_updated_at, '')
  return {
    name,
    root: text(record.root),
    lastActiveAt,
    memoryCount: finiteNumber(record.memoryCount ?? record.observations),
    status: text(record.status, 'local') === 'offline' ? 'offline' : 'local',
  }
}

function normalizeMemory(payload: unknown, projectName: string): MemoryRecord {
  const record = isRecord(payload) ? payload : {}
  const createdAt = text(record.createdAt ?? record.created_at, new Date().toISOString())
  const updatedAt = text(record.updatedAt ?? record.updated_at, createdAt)
  const anchor = text(record.codeAnchor ?? record.code_anchor)
  const score = record.score
  return {
    id: text(record.id, `${projectName}-memory`),
    projectName: text(record.projectName ?? record.project_name, projectName),
    title: text(record.title, 'Untitled memory'),
    content: text(record.content ?? record.preview),
    type: memoryType(record.type),
    scope: memoryScope(record.scope),
    createdAt,
    updatedAt,
    verifyState: verifyState(record.verifyState ?? record.verify_state),
    codeAnchor: anchor || undefined,
    sessionId: text(record.sessionId ?? record.session_id) || undefined,
    score: typeof score === 'number' && Number.isFinite(score) ? score : undefined,
    revisionCount: finiteNumber(record.revisionCount ?? record.revision_count),
    supersededCount: finiteNumber(record.supersededCount ?? record.superseded_count),
    createdBy: text(record.createdBy ?? record.created_by) || undefined,
  }
}

function normalizeGraph(payload: unknown): GraphResponse {
  const record = isRecord(payload) ? payload : {}
  const entities = Array.isArray(record.entities)
    ? record.entities.map((entity) => {
        const item = isRecord(entity) ? entity : {}
        return {
          id: text(item.id, 'unknown-entity'),
          kind: text(item.kind, 'concept'),
          name: text(item.name, 'Unnamed entity'),
          mentionCount: finiteNumber(item.mentionCount ?? item.mention_count),
        }
      })
    : []
  const edges = Array.isArray(record.edges)
    ? record.edges.map((edge) => {
        const item = isRecord(edge) ? edge : {}
        return {
          source: text(item.source ?? item.from_id, 'unknown-entity'),
          target: text(item.target ?? item.to_id, 'unknown-entity'),
          relation: text(item.relation, 'mentions'),
          weight: finiteNumber(item.weight, 1),
        }
      })
    : []
  return {
    seed: text(record.seed, 'memory'),
    seedId: text(record.seedId ?? record.seed_id) || undefined,
    entities,
    edges,
  }
}

function normalizeHealth(payload: unknown): HealthSnapshot {
  const record = isRecord(payload) ? payload : {}
  const state = text(record.state, 'degraded')
  return {
    state: state === 'healthy' ? 'healthy' : state === 'offline' ? 'offline' : 'degraded',
    databaseIntegrity: text(record.database_integrality ?? record.databaseIntegrity, 'unknown'),
    activeObservations: finiteNumber(record.active_observations ?? record.activeObservations),
    softDeletedObservations: finiteNumber(
      record.soft_deleted_observations ?? record.softDeletedObservations,
    ),
    ftsMissing: finiteNumber(record.fts_missing ?? record.ftsMissing),
    embeddingMissing: finiteNumber(record.embedding_missing ?? record.embeddingMissing),
    orphanEmbeddings: finiteNumber(record.orphan_embeddings ?? record.orphanEmbeddings),
    pendingJobs: finiteNumber(record.pending_jobs ?? record.pendingJobs),
    runningJobs: finiteNumber(record.running_jobs ?? record.runningJobs),
    deadJobs: finiteNumber(record.dead_jobs ?? record.deadJobs),
    graphCoverage: finiteNumber(record.graph_coverage ?? record.graphCoverage),
  }
}

function normalizeJob(payload: unknown): JobRecord {
  const record = isRecord(payload) ? payload : {}
  return {
    id: text(record.id, 'unknown-job'),
    kind: text(record.kind, 'unknown'),
    projectName: text(record.project_name ?? record.projectName, ''),
    observationId: text(record.observation_id ?? record.observationId) || undefined,
    status: text(record.status, 'pending'),
    attempts: finiteNumber(record.attempts),
    maxAttempts: finiteNumber(record.max_attempts ?? record.maxAttempts),
    lastError: text(record.last_error ?? record.lastError) || undefined,
    createdAt: text(record.created_at ?? record.createdAt),
    updatedAt: text(record.updated_at ?? record.updatedAt),
  }
}

function normalizeCandidate(payload: unknown): RetrievalCandidate {
  const record = isRecord(payload) ? payload : {}
  return {
    observationId: text(record.observation_id ?? record.observationId, 'unknown-observation'),
    sources: Array.isArray(record.sources) ? record.sources.map((source) => text(source)) : [],
    bm25Rank: finiteNumber(record.bm25_rank ?? record.bm25Rank),
    denseRank: finiteNumber(record.dense_rank ?? record.denseRank),
    fusedRank: finiteNumber(record.fused_rank ?? record.fusedRank),
    finalRank: finiteNumber(record.final_rank ?? record.finalRank),
    score: finiteNumber(record.score),
  }
}

function normalizeGraphStats(payload: unknown): GraphStats {
  const record = isRecord(payload) ? payload : {}
  const counts = (value: unknown) =>
    Array.isArray(value)
      ? value.map((item) => {
          const entry = isRecord(item) ? item : {}
          return { name: text(entry.name), value: finiteNumber(entry.value) }
        })
      : []
  return {
    totalEntities: finiteNumber(record.total_entities ?? record.totalEntities),
    totalMentions: finiteNumber(record.total_mentions ?? record.totalMentions),
    totalEdges: finiteNumber(record.total_edges ?? record.totalEdges),
    coveredObservations: finiteNumber(record.covered_observations ?? record.coveredObservations),
    coverageRatio: finiteNumber(record.coverage_ratio ?? record.coverageRatio),
    entitiesByKind: counts(record.entities_by_kind ?? record.entitiesByKind),
    edgesByRelation: counts(record.edges_by_relation ?? record.edgesByRelation),
  }
}

function normalizeSyncState(payload: unknown): SyncState {
  const record = isRecord(payload) ? payload : {}
  return {
    project: text(record.project, ''),
    lastExportAt: text(record.last_export_at ?? record.lastExportAt) || undefined,
    lastImportAt: text(record.last_import_at ?? record.lastImportAt) || undefined,
    unseenChunkCount: finiteNumber(record.unseen_chunk_count ?? record.unseenChunkCount),
    lastError: text(record.last_error ?? record.lastError) || undefined,
    totalExportedChunks: finiteNumber(record.total_exported_chunks ?? record.totalExportedChunks),
    totalImportedChunks: finiteNumber(record.total_imported_chunks ?? record.totalImportedChunks),
  }
}

export class HttpDashboardApi implements DashboardApi {
  private readonly baseUrl: string
  private readonly fetcher: typeof fetch
  private readonly timeoutMs: number

  constructor(baseUrl = '', fetcher: typeof fetch = globalThis.fetch, timeoutMs = 10_000) {
    this.baseUrl = baseUrl.replace(/\/$/, '')
    this.fetcher = fetcher
    this.timeoutMs = timeoutMs
  }

  listProjects(options?: RequestOptions): Promise<ProjectSummary[]> {
    return this.request<ProjectSummary[] | { projects?: ProjectSummary[] }>(
      '/api/projects',
      { method: 'GET' },
      options,
    )
      .then((payload) => {
        const projects = Array.isArray(payload) ? payload : isRecord(payload) ? payload.projects : []
        return Array.isArray(projects) ? projects.map(normalizeProjectSummary) : []
      })
      .catch((error: unknown) => {
        if (error instanceof DashboardApiError && (error.status === 404 || error.status === 501)) {
          return []
        }
        throw error
      })
  }

  getProjectOverview(projectName: string, options?: RequestOptions): Promise<ProjectOverview> {
    return this.request<StatsPayload>(
      '/api/stats',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      },
      options,
    ).then((payload): ProjectOverview => {
      const summary = isRecord(payload.summary) ? payload.summary : {}
      const healthPayload = normalizeHealth(payload.health)
      const totalMemories = Math.max(
        0,
        finiteNumber(summary.observations ?? payload.observations_scanned),
      )
      const openConflicts = Math.max(0, finiteNumber(payload.consolidation_proposals))
      const now = new Date().toISOString()
      const lastIndexedAt = text(
        summary.latest_updated_at ?? summary.latest_observation_at,
        now,
      )
      const stats: DashboardStats = {
        totalMemories,
        addedThisWeek: 0,
        verifiedPercent: 0,
        openConflicts,
        indexedEntities: finiteNumber(summary.entities),
        indexCoveragePercent: Math.max(0, Math.min(100, healthPayload.graphCoverage * 100)),
        lastIndexedAt,
      }
      return {
        project: {
          name: projectName,
          root: '',
          lastActiveAt: lastIndexedAt,
          memoryCount: totalMemories,
          status: healthPayload.state === 'offline' ? 'offline' : 'local',
        },
        stats,
        health: {
          state: healthPayload.state,
          daemon: 'statefulmemory-daemon',
          socket: 'uds://127.0.0.1/statefulmemory.sock',
          pendingWrites: healthPayload.pendingJobs + healthPayload.runningJobs,
          lastSyncAt: now,
        },
      }
    })
  }

  listMemories(projectName: string, options?: RequestOptions): Promise<MemoryListResponse> {
    return this.request<MemoryPayload[] | MemoryListPayload>(
      `/api/projects/${encodeURIComponent(projectName)}/memories?limit=50`,
      { method: 'GET' },
      options,
    )
      .then((payload) => {
        if (Array.isArray(payload)) {
          return { memories: payload.map((memory) => normalizeMemory(memory, projectName)) }
        }
        return {
          memories: (payload.memories ?? payload.observations ?? []).map((memory) =>
            normalizeMemory(memory, projectName),
          ),
          nextCursor: payload.nextCursor ?? payload.next_cursor,
        }
      })
      .catch((error: unknown) => {
        if (error instanceof DashboardApiError && (error.status === 404 || error.status === 501)) {
          return { memories: [] }
        }
        throw error
      })
  }

  searchMemories(
    projectName: string,
    query: string,
    options?: RequestOptions,
  ): Promise<SearchResponse> {
    return this.request<SearchPayload | MemoryPayload[]>(
      '/api/search',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ query, limit: 15 }),
      },
      options,
    ).then((payload) => {
      if (Array.isArray(payload)) {
        return { memories: payload.map((memory) => normalizeMemory(memory, projectName)) }
      }
      return {
        memories: (payload.memories ?? payload.hits ?? []).map((memory) =>
          normalizeMemory(memory, projectName),
        ),
        warning: payload.warning,
        tokensUsed: payload.tokensUsed ?? payload.tokens_used,
      }
    })
  }

  getGraph(
    _projectName: string,
    entity: string,
    hops: number,
    options?: RequestOptions,
  ): Promise<GraphResponse> {
    return this.request<unknown>(
      '/api/graph',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ entity, hops }),
      },
      options,
    ).then(normalizeGraph)
  }

  getObservationDetail(
    projectName: string,
    observationId: string,
    options?: RequestOptions,
  ): Promise<ObservationDetail> {
    return this.request<Record<string, unknown>>(
      '/api/observation',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ observation_id: Number(observationId) }),
      },
      options,
    ).then((payload) => {
      const observation = normalizeMemory(payload.observation, projectName)
      return {
        observation,
        anchors: Array.isArray(payload.anchors)
          ? payload.anchors.filter(isRecord)
          : [],
        facts: Array.isArray(payload.facts) ? payload.facts.filter(isRecord) : [],
        relations: Array.isArray(payload.relations) ? payload.relations.filter(isRecord) : [],
        entities: normalizeGraph({ entities: payload.entities }).entities,
        edges: normalizeGraph({ edges: payload.edges }).edges,
        history: Array.isArray(payload.history)
          ? payload.history.map((item) => normalizeMemory(item, projectName))
          : [],
      }
    })
  }

  explainRetrieval(
    projectName: string,
    query: string,
    limit: number,
    mode: string,
    options?: RequestOptions,
  ): Promise<RetrievalExplanation> {
    return this.request<Record<string, unknown>>(
      '/api/retrieval/explain',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ query, limit, mode, rerank: '' }),
      },
      options,
    ).then((payload) => ({
      query: text(payload.query, query),
      mode: text(payload.mode, mode),
      rerank: text(payload.rerank),
      candidateDepth: finiteNumber(payload.candidate_depth ?? payload.candidateDepth),
      rerankTimedOut: Boolean(payload.rerank_timed_out ?? payload.rerankTimedOut),
      elapsedUs: finiteNumber(payload.elapsed_us ?? payload.elapsedUs),
      candidates: Array.isArray(payload.candidates)
        ? payload.candidates.map(normalizeCandidate)
        : [],
      results: Array.isArray(payload.results)
        ? payload.results.map((item) => normalizeMemory(item, projectName))
        : [],
    }))
  }

  getJobs(projectName: string, options?: RequestOptions): Promise<JobRecord[]> {
    void projectName
    return this.request<unknown>(
      '/api/jobs',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      },
      options,
    ).then((payload) => {
      const record = isRecord(payload) ? payload : {}
      const jobs = Array.isArray(record.jobs) ? record.jobs : []
      return jobs.map(normalizeJob)
    })
  }

  getGraphStats(projectName: string, options?: RequestOptions): Promise<GraphStats> {
    void projectName
    return this.request<Record<string, unknown>>(
      '/api/visualization/entities',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      },
      options,
    ).then((payload) => normalizeGraphStats(isRecord(payload.graph) ? payload.graph : payload))
  }

  getSyncState(projectName: string, options?: RequestOptions): Promise<SyncState> {
    void projectName
    return this.request<unknown>(
      '/api/sync',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      },
      options,
    ).then(normalizeSyncState)
  }

  private async request<T>(
    path: string,
    init: RequestInit,
    options: RequestOptions = {},
  ): Promise<T> {
    const controller = new AbortController()
    const externalSignal = options.signal
    const abortFromCaller = () => controller.abort(externalSignal?.reason)
    let timedOut = false

    if (externalSignal?.aborted) {
      throw abortError(externalSignal)
    }
    externalSignal?.addEventListener('abort', abortFromCaller, { once: true })
    const timer = setTimeout(() => {
      timedOut = true
      controller.abort()
    }, this.timeoutMs)

    let response: Response
    try {
      response = await this.fetcher(`${this.baseUrl}${path}`, {
        ...init,
        headers: {
          Accept: 'application/json',
          ...(init.headers ?? {}),
        },
        signal: controller.signal,
        credentials: 'include',
      })
    } catch (cause) {
      if (externalSignal?.aborted) {
        throw new DashboardApiError('Request cancelled', 0, 'request_cancelled', cause)
      }
      if (timedOut) {
        throw new DashboardApiError('Request timed out', 0, 'timeout', cause)
      }
      throw new DashboardApiError('Unable to reach the local dashboard API', 0, 'network_error', cause)
    } finally {
      clearTimeout(timer)
      externalSignal?.removeEventListener('abort', abortFromCaller)
    }

    let bodyText: string
    try {
      bodyText = await response.text()
    } catch (cause) {
      throw new DashboardApiError(
        'Unable to read the local dashboard API response',
        response.status,
        'response_error',
        cause,
      )
    }

    let body: unknown
    if (bodyText) {
      try {
        body = JSON.parse(bodyText) as unknown
      } catch (cause) {
        throw new DashboardApiError(
          'The local dashboard API returned invalid JSON',
          response.status,
          'invalid_json',
          cause,
        )
      }
    }

    if (!response.ok) {
      throw new DashboardApiError(
        errorMessage(body, `Dashboard API request failed (${response.status})`),
        response.status,
        `http_${response.status}`,
      )
    }

    if (response.status === 204 || bodyText.length === 0) {
      return undefined as T
    }
    return body as T
  }
}
