export type MemoryType = 'decision' | 'pattern' | 'fix' | 'preference' | 'context' | 'fact'

export type VerifyState = 'verified' | 'unanchored' | 'stale' | 'invalidated' | 'unprovable'

export interface MemoryRecord {
  id: string
  projectName: string
  title: string
  content: string
  type: MemoryType
  scope: 'project' | 'personal' | 'team'
  createdAt: string
  updatedAt: string
  verifyState: VerifyState
  codeAnchor?: string
  sessionId?: string
  score?: number
  revisionCount: number
  supersededCount: number
  createdBy?: string
}

export interface ProjectSummary {
  name: string
  root: string
  lastActiveAt: string
  memoryCount?: number
  status?: 'local' | 'syncing' | 'offline'
}

export interface DashboardStats {
  totalMemories: number
  addedThisWeek: number
  verifiedPercent: number
  openConflicts: number
  indexedEntities: number
  indexCoveragePercent: number
  lastIndexedAt: string
}

export interface IndexHealth {
  state: 'healthy' | 'degraded' | 'offline'
  daemon: string
  socket: string
  pendingWrites: number
  lastSyncAt: string
}

export interface ProjectOverview {
  project: ProjectSummary
  stats: DashboardStats
  health: IndexHealth
}

export interface MemoryListResponse {
  memories: MemoryRecord[]
  nextCursor?: string
}

export interface SearchResponse {
  memories: MemoryRecord[]
  warning?: string
  tokensUsed?: number
}

export interface GraphEntity {
  id: string
  kind: string
  name: string
  mentionCount: number
}

export interface GraphEdge {
  source: string
  target: string
  relation: string
  weight: number
}

export interface GraphResponse {
  seed: string
  seedId?: string
  entities: GraphEntity[]
  edges: GraphEdge[]
}

export interface HealthSnapshot {
  state: 'healthy' | 'degraded' | 'offline'
  databaseIntegrity: string
  activeObservations: number
  softDeletedObservations: number
  ftsMissing: number
  embeddingMissing: number
  orphanEmbeddings: number
  pendingJobs: number
  runningJobs: number
  deadJobs: number
  graphCoverage: number
}

export interface JobRecord {
  id: string
  kind: string
  projectName: string
  observationId?: string
  status: string
  attempts: number
  maxAttempts: number
  lastError?: string
  createdAt: string
  updatedAt: string
}

export interface RetrievalCandidate {
  observationId: string
  sources: string[]
  bm25Rank: number
  denseRank: number
  fusedRank: number
  finalRank: number
  score: number
}

export interface RetrievalExplanation {
  query: string
  mode: string
  rerank: string
  candidateDepth: number
  rerankTimedOut: boolean
  elapsedUs: number
  candidates: RetrievalCandidate[]
  results: MemoryRecord[]
}

export interface ObservationDetail {
  observation: MemoryRecord
  anchors: Array<Record<string, unknown>>
  facts: Array<Record<string, unknown>>
  relations: Array<Record<string, unknown>>
  entities: GraphEntity[]
  edges: GraphEdge[]
  history: MemoryRecord[]
}

export interface GraphStats {
  totalEntities: number
  totalMentions: number
  totalEdges: number
  coveredObservations: number
  coverageRatio: number
  entitiesByKind: Array<{ name: string; value: number }>
  edgesByRelation: Array<{ name: string; value: number }>
}

export interface SyncState {
  project: string
  lastExportAt?: string
  lastImportAt?: string
  unseenChunkCount: number
  lastError?: string
  totalExportedChunks: number
  totalImportedChunks: number
}

export interface RequestOptions {
  signal?: AbortSignal
}

export interface DashboardApi {
  listProjects(options?: RequestOptions): Promise<ProjectSummary[]>
  getProjectOverview(projectName: string, options?: RequestOptions): Promise<ProjectOverview>
  listMemories(projectName: string, options?: RequestOptions): Promise<MemoryListResponse>
  searchMemories(
    projectName: string,
    query: string,
    options?: RequestOptions,
  ): Promise<SearchResponse>
  getGraph(
    projectName: string,
    entity: string,
    hops: number,
    options?: RequestOptions,
  ): Promise<GraphResponse>
  getObservationDetail(
    projectName: string,
    observationId: string,
    options?: RequestOptions,
  ): Promise<ObservationDetail>
  explainRetrieval(
    projectName: string,
    query: string,
    limit: number,
    mode: string,
    options?: RequestOptions,
  ): Promise<RetrievalExplanation>
  getJobs(projectName: string, options?: RequestOptions): Promise<JobRecord[]>
  getGraphStats(projectName: string, options?: RequestOptions): Promise<GraphStats>
  getSyncState(projectName: string, options?: RequestOptions): Promise<SyncState>
}
