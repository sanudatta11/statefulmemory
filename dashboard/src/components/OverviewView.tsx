import type { DashboardStats, IndexHealth, MemoryRecord } from '../api/types'
import { Icon } from './Icons'
import { MemoryExplorer } from './MemoryExplorer'
import { OverviewCards } from './OverviewCards'
import { StatusDot, StatusPill } from './StateViews'
import { formatCompact, formatNumber, formatRelativeTime } from '../utils/format'

interface OverviewViewProps {
  projectName: string
  stats: DashboardStats | undefined
  health: IndexHealth | undefined
  memories: MemoryRecord[]
  loading: boolean
  error: Error | null
  onRetry: () => void
  onSelectMemory: (memory: MemoryRecord) => void
  onSearch: (query: string) => void
  searchResults: MemoryRecord[] | null
  searchStatus: 'idle' | 'loading' | 'success' | 'error'
  searchError: Error | null
  onOpenMemory: () => void
  onOpenGraph: () => void
  onOpenRetrieval: () => void
}

export function OverviewView({
  projectName,
  stats,
  health,
  memories,
  loading,
  error,
  onRetry,
  onSelectMemory,
  onSearch,
  searchResults,
  searchStatus,
  searchError,
  onOpenMemory,
  onOpenGraph,
  onOpenRetrieval,
}: OverviewViewProps) {
  return (
    <div className="page-stack">
      <div className="page-heading page-heading--overview">
        <div>
          <div className="eyebrow"><span className="eyebrow__pulse" /> Local project workspace</div>
          <h1>Memory, made legible.</h1>
          <p>A calm view of what your agents know, what changed, and what needs a human look.</p>
        </div>
        <div className="page-heading__actions">
          <span className="last-sync"><span className="status-dot status-dot--healthy" />Synced {stats ? formatRelativeTime(stats.lastIndexedAt) : 'locally'}</span>
          <button className="button button--primary" onClick={onOpenMemory} type="button"><Icon name="plus" size={16} />Add memory</button>
        </div>
      </div>

      <OverviewCards loading={loading} stats={stats} />

      <div className="overview-content-grid">
        <MemoryExplorer
          compact
          error={error}
          loading={loading}
          memories={memories}
          onRetry={onRetry}
          onSearch={onSearch}
          onSelect={onSelectMemory}
          projectName={projectName}
          searchError={searchError}
          searchResults={searchResults}
          searchStatus={searchStatus}
        />
        <aside className="overview-side-stack">
          <section className="panel signal-panel">
            <div className="panel__header panel__header--small">
              <div><h2>Retrieval signal</h2><p>Last 24 hours · local</p></div>
              <StatusPill tone="healthy">stable</StatusPill>
            </div>
            <div className="signal-score"><strong>0.84</strong><span>mean confidence</span></div>
            <div className="signal-bars" aria-label="Retrieval signal by stage">
              <div><span>BM25</span><i><b style={{ width: '88%' }} /></i><strong>88</strong></div>
              <div><span>Dense</span><i><b style={{ width: '76%' }} /></i><strong>76</strong></div>
              <div><span>Rerank</span><i><b style={{ width: '63%' }} /></i><strong>63</strong></div>
            </div>
            <button className="text-button" onClick={onOpenRetrieval} type="button">Open retrieval lab <Icon name="arrow-up-right" size={14} /></button>
          </section>

          <section className="panel index-panel">
            <div className="panel__header panel__header--small">
              <div><h2>Index health</h2><p>Local mirrors</p></div>
              <StatusDot tone={health?.state === 'healthy' ? 'healthy' : 'warning'} />
            </div>
            <div className="index-row"><span><Icon name="database" size={15} />SQLite + FTS5</span><strong>ready</strong></div>
            <div className="index-row"><span><Icon name="spark" size={15} />sqlite-vec</span><strong>ready</strong></div>
            <div className="index-row"><span><Icon name="graph" size={15} />Entity graph</span><strong>{stats ? `${stats.indexCoveragePercent.toFixed(1)}%` : '—'}</strong></div>
            <div className="coverage-meter"><span style={{ width: `${stats?.indexCoveragePercent ?? 0}%` }} /></div>
            <button className="text-button" onClick={onOpenGraph} type="button">Inspect graph <Icon name="arrow-up-right" size={14} /></button>
          </section>

          <section className="panel local-note">
            <span className="local-note__icon"><Icon name="shield" size={17} /></span>
            <div><strong>Local by default</strong><p>Records stay on this machine. No search key or cloud account required.</p></div>
            <button aria-label="Learn about local-first memory" className="icon-button icon-button--small" type="button"><Icon name="arrow-up-right" size={14} /></button>
          </section>
        </aside>
      </div>

      <section className="activity-strip">
        <div className="activity-strip__heading"><div className="eyebrow"><Icon name="activity" size={13} /> Project pulse</div><h2>Small signals worth keeping</h2></div>
        <div className="activity-strip__items">
          <div className="activity-item"><span className="activity-item__icon activity-item__icon--green"><Icon name="check" size={14} /></span><div><strong>{stats ? `${stats.verifiedPercent.toFixed(1)}%` : '—'} verified</strong><span>Anchors match current code</span></div></div>
          <div className="activity-item"><span className="activity-item__icon activity-item__icon--copper"><Icon name="warning" size={14} /></span><div><strong>{stats ? stats.openConflicts : '—'} conflicts</strong><span>Supersession review queue</span></div></div>
          <div className="activity-item"><span className="activity-item__icon activity-item__icon--blue"><Icon name="graph" size={14} /></span><div><strong>{stats ? formatCompact(stats.indexedEntities) : '—'} entities</strong><span>Available for graph expansion</span></div></div>
          <div className="activity-item"><span className="activity-item__icon activity-item__icon--blue"><Icon name="clock" size={14} /></span><div><strong>{formatNumber(memories.length)} loaded</strong><span>Ready in this workspace</span></div></div>
        </div>
      </section>
    </div>
  )
}
