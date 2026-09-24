import type {
  DashboardApi,
  DashboardStats,
  GraphResponse,
  GraphStats,
  JobRecord,
  MemoryListResponse,
  MemoryRecord,
  ObservationDetail,
  ProjectOverview,
  ProjectSummary,
  RequestOptions,
  RetrievalExplanation,
  SearchResponse,
  SyncState,
} from '../api/types'

const projects: ProjectSummary[] = [
  {
    name: 'memlayer',
    root: '~/Documents/memlayer',
    lastActiveAt: '2026-09-25T09:42:00.000Z',
    memoryCount: 12482,
    status: 'local',
  },
  {
    name: 'atlas-api',
    root: '~/Work/atlas-api',
    lastActiveAt: '2026-09-24T16:18:00.000Z',
    memoryCount: 6821,
    status: 'local',
  },
  {
    name: 'laya-lab',
    root: '~/Work/laya-lab',
    lastActiveAt: '2026-09-22T11:06:00.000Z',
    memoryCount: 3190,
    status: 'local',
  },
]

const stats: Record<string, DashboardStats> = {
  memlayer: {
    totalMemories: 12482,
    addedThisWeek: 184,
    verifiedPercent: 96.8,
    openConflicts: 7,
    indexedEntities: 2846,
    indexCoveragePercent: 98.2,
    lastIndexedAt: '2026-09-25T09:40:00.000Z',
  },
  'atlas-api': {
    totalMemories: 6821,
    addedThisWeek: 73,
    verifiedPercent: 94.1,
    openConflicts: 3,
    indexedEntities: 1420,
    indexCoveragePercent: 97.5,
    lastIndexedAt: '2026-09-24T16:16:00.000Z',
  },
  'laya-lab': {
    totalMemories: 3190,
    addedThisWeek: 41,
    verifiedPercent: 98.4,
    openConflicts: 1,
    indexedEntities: 806,
    indexCoveragePercent: 99.1,
    lastIndexedAt: '2026-09-22T11:03:00.000Z',
  },
}

const memories: Record<string, MemoryRecord[]> = {
  memlayer: [
    {
      id: 'mem_8f31',
      projectName: 'memlayer',
      title: 'Keep database writes on the project write thread',
      content: 'All SQLite mutations, including graph indexing, cross the per-project write thread through WriteRequest.',
      type: 'decision',
      scope: 'project',
      createdAt: '2026-09-25T08:58:00.000Z',
      updatedAt: '2026-09-25T09:12:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'statefulmemory-storage/src/lib.rs::write_request',
      sessionId: 'session_7a2c',
      score: 0.98,
      revisionCount: 2,
      supersededCount: 0,
      createdBy: 'opencode',
    },
    {
      id: 'mem_8efc',
      projectName: 'memlayer',
      title: 'FTS key expansion is live on migration V12',
      content: 'The observations_fts rebuild adds key_expand so fact keys participate in lexical retrieval.',
      type: 'fix',
      scope: 'project',
      createdAt: '2026-09-25T07:41:00.000Z',
      updatedAt: '2026-09-25T08:04:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'migrations/V12__fact_key_expand.sql::observations_fts',
      sessionId: 'session_3bd1',
      score: 0.95,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'cursor',
    },
    {
      id: 'mem_8d90',
      projectName: 'memlayer',
      title: 'Graph expansion remains budget capped',
      content: 'Entity graph hits cannot displace primary retrieval results; expansion receives a percentage of max_tokens.',
      type: 'pattern',
      scope: 'project',
      createdAt: '2026-09-24T15:27:00.000Z',
      updatedAt: '2026-09-24T15:30:00.000Z',
      verifyState: 'unanchored',
      codeAnchor: 'statefulmemory-retrieval/src/lib.rs::graph_expand',
      sessionId: 'session_91aa',
      score: 0.91,
      revisionCount: 3,
      supersededCount: 1,
      createdBy: 'codex',
    },
    {
      id: 'mem_8c12',
      projectName: 'memlayer',
      title: 'Use local CE reranking by default',
      content: 'Local cross-encoder reranking stays on for hybrid search while LLM features remain opt-in.',
      type: 'preference',
      scope: 'project',
      createdAt: '2026-09-23T12:10:00.000Z',
      updatedAt: '2026-09-23T12:11:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'statefulmemory-core/src/config.rs::rerank',
      sessionId: 'session_4e10',
      score: 0.89,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'opencode',
    },
    {
      id: 'mem_8ab4',
      projectName: 'memlayer',
      title: 'Laya sidecar is optional loopback infrastructure',
      content: 'Typed router, Decide, and conflict signals can use a local sidecar; search and embedding stay local.',
      type: 'context',
      scope: 'project',
      createdAt: '2026-09-22T09:33:00.000Z',
      updatedAt: '2026-09-22T10:02:00.000Z',
      verifyState: 'stale',
      codeAnchor: 'statefulmemory-daemon/src/laya.rs::spawn',
      sessionId: 'session_9fd3',
      score: 0.84,
      revisionCount: 4,
      supersededCount: 2,
      createdBy: 'gemini',
    },
    {
      id: 'mem_8791',
      projectName: 'memlayer',
      title: 'PPR is the default graph ranker when enabled',
      content: 'Graph expansion uses HippoRAG-style personalized PageRank over typed entity edges.',
      type: 'fact',
      scope: 'project',
      createdAt: '2026-09-20T18:21:00.000Z',
      updatedAt: '2026-09-21T08:14:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'statefulmemory-storage/src/graph.rs::ppr',
      sessionId: 'session_5d03',
      score: 0.82,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'opencode',
    },
  ],
  'atlas-api': [
    {
      id: 'atlas_2c9a',
      projectName: 'atlas-api',
      title: 'Paginate list endpoints with opaque cursors',
      content: 'Clients should treat cursor tokens as opaque and preserve them when requesting the next page.',
      type: 'decision',
      scope: 'project',
      createdAt: '2026-09-24T15:53:00.000Z',
      updatedAt: '2026-09-24T16:18:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'src/routes/observations.rs::list',
      sessionId: 'atlas_4a1',
      score: 0.96,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'opencode',
    },
    {
      id: 'atlas_2bd1',
      projectName: 'atlas-api',
      title: 'Return conflict state alongside Decide evidence',
      content: 'A recommendation is easier to audit when open and resolved conflicts are returned with evidence.',
      type: 'pattern',
      scope: 'project',
      createdAt: '2026-09-23T13:45:00.000Z',
      updatedAt: '2026-09-23T14:02:00.000Z',
      verifyState: 'unanchored',
      codeAnchor: 'src/services/decide.rs::response',
      sessionId: 'atlas_6d8',
      score: 0.9,
      revisionCount: 2,
      supersededCount: 0,
      createdBy: 'cursor',
    },
    {
      id: 'atlas_29d0',
      projectName: 'atlas-api',
      title: 'Daemon health should separate socket from liveness',
      content: 'A running daemon can still report a degraded index; surface those states independently.',
      type: 'context',
      scope: 'project',
      createdAt: '2026-09-21T10:12:00.000Z',
      updatedAt: '2026-09-21T11:09:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'src/ops/health.rs::status',
      sessionId: 'atlas_1f0',
      score: 0.86,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'codex',
    },
  ],
  'laya-lab': [
    {
      id: 'laya_88d2',
      projectName: 'laya-lab',
      title: 'Measure router latency before changing model',
      content: 'Compare p50 and p95 sidecar latency with the same workload before interpreting quality changes.',
      type: 'decision',
      scope: 'project',
      createdAt: '2026-09-22T10:21:00.000Z',
      updatedAt: '2026-09-22T11:06:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'bench/router_latency.py::measure',
      sessionId: 'laya_7b0',
      score: 0.94,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'opencode',
    },
    {
      id: 'laya_84aa',
      projectName: 'laya-lab',
      title: 'Teacher traces need stable request identifiers',
      content: 'Stable identifiers make sidecar teacher traces joinable with dashboard retrieval events.',
      type: 'fix',
      scope: 'project',
      createdAt: '2026-09-21T08:19:00.000Z',
      updatedAt: '2026-09-21T09:02:00.000Z',
      verifyState: 'unanchored',
      codeAnchor: 'src/sidecar/server.py::request_id',
      sessionId: 'laya_2c4',
      score: 0.88,
      revisionCount: 2,
      supersededCount: 0,
      createdBy: 'gemini',
    },
    {
      id: 'laya_7fe1',
      projectName: 'laya-lab',
      title: 'Sidecar health endpoint reports active backend',
      content: 'Health output should identify whether the active backend is MLX, PyTorch, or unavailable.',
      type: 'fact',
      scope: 'project',
      createdAt: '2026-09-19T16:44:00.000Z',
      updatedAt: '2026-09-20T07:15:00.000Z',
      verifyState: 'verified',
      codeAnchor: 'src/sidecar/server.py::health',
      sessionId: 'laya_5a8',
      score: 0.81,
      revisionCount: 1,
      supersededCount: 0,
      createdBy: 'opencode',
    },
  ],
}

function copyProject(project: ProjectSummary): ProjectSummary {
  return { ...project }
}

function copyMemory(memory: MemoryRecord): MemoryRecord {
  return { ...memory }
}

function copyStats(projectName: string): DashboardStats {
  const projectStats = stats[projectName] ?? stats.memlayer
  return { ...projectStats }
}

function projectFor(projectName: string): ProjectSummary {
  const project = projects.find((candidate) => candidate.name === projectName) ?? projects[0]
  return copyProject(project)
}

function projectMemories(projectName: string): MemoryRecord[] {
  return (memories[projectName] ?? memories.memlayer).map(copyMemory)
}

function waitFor(signal: AbortSignal | undefined, latencyMs: number): Promise<void> {
  if (!latencyMs) {
    return Promise.resolve()
  }
  return new Promise((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout>
    const onAbort = () => {
      clearTimeout(timer)
      reject(new DOMException('Request cancelled', 'AbortError'))
    }
    if (signal?.aborted) {
      onAbort()
      return
    }
    timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort)
      resolve()
    }, latencyMs)
    signal?.addEventListener('abort', onAbort, { once: true })
  })
}

export class PreviewDashboardApi implements DashboardApi {
  constructor(private readonly latencyMs = 280) {}

  async listProjects(options?: RequestOptions): Promise<ProjectSummary[]> {
    await waitFor(options?.signal, this.latencyMs)
    return projects.map(copyProject)
  }

  async getProjectOverview(projectName: string, options?: RequestOptions): Promise<ProjectOverview> {
    await waitFor(options?.signal, this.latencyMs)
    const project = projectFor(projectName)
    return {
      project,
      stats: copyStats(project.name),
      health: {
        state: 'healthy',
        daemon: 'statefulmemory-daemon',
        socket: 'uds://127.0.0.1/statefulmemory.sock',
        pendingWrites: 0,
        lastSyncAt: '2026-09-25T09:40:00.000Z',
      },
    }
  }

  async listMemories(projectName: string, options?: RequestOptions): Promise<MemoryListResponse> {
    await waitFor(options?.signal, this.latencyMs)
    return { memories: projectMemories(projectName).slice(0, 50) }
  }

  async searchMemories(
    projectName: string,
    query: string,
    options?: RequestOptions,
  ): Promise<SearchResponse> {
    await waitFor(options?.signal, this.latencyMs)
    const normalizedQuery = query.trim().toLowerCase()
    if (!normalizedQuery) {
      return { memories: projectMemories(projectName).slice(0, 50) }
    }
    const results = projectMemories(projectName)
      .filter((memory) => {
        const haystack = [
          memory.title,
          memory.content,
          memory.codeAnchor ?? '',
          memory.type,
        ]
          .join(' ')
          .toLowerCase()
        return haystack.includes(normalizedQuery)
      })
      .map((memory) => ({ ...memory, score: memory.score ?? 0.7 }))
    return { memories: results, tokensUsed: Math.ceil(results.length * 84) }
  }

  async getGraph(
    projectName: string,
    entity: string,
    hops: number,
    options?: RequestOptions,
  ): Promise<GraphResponse> {
    await waitFor(options?.signal, this.latencyMs)
    const baseEntities = [
      { id: `${projectName}-entity`, kind: 'concept', name: entity || 'memory', mentionCount: 42 },
      { id: `${projectName}-file`, kind: 'file', name: 'src/retrieval/mod.rs', mentionCount: 18 },
      { id: `${projectName}-symbol`, kind: 'symbol', name: 'hybrid_search', mentionCount: 13 },
      { id: `${projectName}-agent`, kind: 'agent', name: 'opencode', mentionCount: 9 },
      { id: `${projectName}-policy`, kind: 'concept', name: 'local-first', mentionCount: 7 },
    ]
    const entities = hops > 1 ? baseEntities : baseEntities.slice(0, 4)
    const edges = entities.slice(1).map((target, index) => ({
      source: entities[0].id,
      target: target.id,
      relation: ['mentions', 'implements', 'uses', 'follows'][index] ?? 'mentions',
      weight: Math.max(0.35, 0.88 - index * 0.13),
    }))
    return { seed: entity || 'memory', seedId: entities[0]?.id, entities, edges }
  }

  async getObservationDetail(
    projectName: string,
    observationId: string,
    options?: RequestOptions,
  ): Promise<ObservationDetail> {
    await waitFor(options?.signal, this.latencyMs)
    const observation = projectMemories(projectName).find((memory) => memory.id === observationId) ?? projectMemories(projectName)[0]
    if (!observation) throw new Error('Observation not found')
    return {
      observation,
      anchors: observation.codeAnchor
        ? [{ path: observation.codeAnchor.split('::')[0], symbol: observation.codeAnchor.split('::')[1] }]
        : [],
      facts: [],
      relations: [],
      entities: [],
      edges: [],
      history: [observation],
    }
  }

  async explainRetrieval(
    projectName: string,
    query: string,
    limit: number,
    mode: string,
    options?: RequestOptions,
  ): Promise<RetrievalExplanation> {
    await waitFor(options?.signal, this.latencyMs)
    const results = await this.searchMemories(projectName, query, options)
    const candidates = results.memories.slice(0, limit).map((memory, index) => ({
      observationId: memory.id,
      sources: index % 2 === 0 ? ['bm25', 'dense'] : ['bm25'],
      bm25Rank: index + 1,
      denseRank: index % 2 === 0 ? index + 1 : 0,
      fusedRank: index + 1,
      finalRank: index + 1,
      score: memory.score ?? 0,
    }))
    return {
      query,
      mode,
      rerank: 'local',
      candidateDepth: candidates.length,
      rerankTimedOut: false,
      elapsedUs: 4200,
      candidates,
      results: results.memories.slice(0, limit),
    }
  }

  async getJobs(projectName: string, options?: RequestOptions): Promise<JobRecord[]> {
    await waitFor(options?.signal, this.latencyMs)
    void projectName
    return [
      {
        id: 'preview-job',
        kind: 'embed',
        projectName,
        status: 'pending',
        attempts: 0,
        maxAttempts: 5,
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      },
    ]
  }

  async getGraphStats(projectName: string, options?: RequestOptions): Promise<GraphStats> {
    await waitFor(options?.signal, this.latencyMs)
    void projectName
    return {
      totalEntities: 2846,
      totalMentions: 9120,
      totalEdges: 6412,
      coveredObservations: 12244,
      coverageRatio: 0.982,
      entitiesByKind: [
        { name: 'concept', value: 1840 },
        { name: 'file', value: 612 },
        { name: 'symbol', value: 394 },
      ],
      edgesByRelation: [
        { name: 'mentions', value: 5200 },
        { name: 'implements', value: 1212 },
      ],
    }
  }

  async getSyncState(projectName: string, options?: RequestOptions): Promise<SyncState> {
    await waitFor(options?.signal, this.latencyMs)
    return {
      project: projectName,
      lastExportAt: '2026-09-25T09:40:00.000Z',
      lastImportAt: '2026-09-25T09:41:00.000Z',
      unseenChunkCount: 0,
      totalExportedChunks: 42,
      totalImportedChunks: 42,
    }
  }
}

export function previewProjects(): ProjectSummary[] {
  return projects.map(copyProject)
}
