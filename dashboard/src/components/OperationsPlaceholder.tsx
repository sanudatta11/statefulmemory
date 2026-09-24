import { useEffect, useState } from 'react'
import { Icon, type IconName } from './Icons'
import { StatusDot, StatusPill } from './StateViews'
import type { DashboardApi, IndexHealth, JobRecord, SyncState } from '../api/types'

interface OperationsPlaceholderProps {
  projectName: string
  api: DashboardApi
  health: IndexHealth | undefined
  onRefresh: () => void
}

interface CheckItem {
  label: string
  detail: string
  value: string
  icon: IconName
  tone: 'healthy' | 'warning'
}

export function OperationsPlaceholder({ projectName, api, health, onRefresh }: OperationsPlaceholderProps) {
  const [jobs, setJobs] = useState<JobRecord[]>([])
  const [sync, setSync] = useState<SyncState | null>(null)
  const [error, setError] = useState<Error | null>(null)

  useEffect(() => {
    let active = true
    setError(null)
    void Promise.all([api.getJobs(projectName), api.getSyncState(projectName)]).then(
      ([nextJobs, nextSync]) => {
        if (!active) return
        setJobs(nextJobs)
        setSync(nextSync)
      },
      (reason: unknown) => {
        if (active) setError(reason instanceof Error ? reason : new Error('Operations data unavailable'))
      },
    )
    return () => {
      active = false
    }
  }, [api, onRefresh, projectName])

  const checks: CheckItem[] = [
    { label: 'Daemon', detail: 'RPC process', value: health?.daemon ?? 'checking', icon: 'server', tone: health?.state === 'offline' ? 'warning' : 'healthy' },
    { label: 'Write queue', detail: 'pending mutations', value: health ? String(health.pendingWrites) : '—', icon: 'database', tone: health && health.pendingWrites > 8 ? 'warning' : 'healthy' },
    { label: 'FTS index', detail: 'lexical mirror', value: health?.state === 'degraded' ? 'degraded' : 'ready', icon: 'search', tone: health?.state === 'degraded' ? 'warning' : 'healthy' },
    { label: 'Embedding index', detail: 'sqlite-vec mirror', value: health?.state === 'healthy' ? 'ready' : 'checking', icon: 'spark', tone: health?.state === 'healthy' ? 'healthy' : 'warning' },
  ]
  const pendingJobs = jobs.filter((job) => job.status === 'pending').length
  const deadJobs = jobs.filter((job) => job.status === 'dead').length

  return (
    <div className="placeholder-page">
      <div className="page-heading">
        <div>
          <div className="eyebrow"><Icon name="activity" size={13} /> Local operations</div>
          <h1>Operations</h1>
          <p>Keep daemon, indexes, and background workers observable without leaving localhost.</p>
        </div>
        <button className="button button--quiet" onClick={onRefresh} type="button"><Icon name="refresh" size={15} />Refresh checks</button>
      </div>

      <div className="operations-banner">
        <span className="operations-banner__mark"><StatusDot tone={health?.state === 'degraded' || deadJobs ? 'warning' : 'healthy'} /></span>
        <div><strong>{health?.state === 'degraded' || deadJobs ? 'Some local services need attention' : 'Local-first services are healthy'}</strong><span>{health?.socket ?? 'Checking the local socket…'}</span></div>
        <StatusPill icon={health?.state === 'healthy' && !deadJobs ? 'check' : 'warning'} tone={health?.state === 'healthy' && !deadJobs ? 'healthy' : 'warning'}>{deadJobs ? `${deadJobs} dead` : health?.state ?? 'checking'}</StatusPill>
      </div>

      <section className="operations-grid">
        {checks.map((check) => (
          <article className="operation-check" key={check.label}>
            <div className="operation-check__top"><span className="operation-check__icon"><Icon name={check.icon} size={17} /></span><StatusDot tone={check.tone} /></div>
            <strong>{check.label}</strong>
            <span>{check.detail}</span>
            <div className="operation-check__value mono">{check.value}</div>
          </article>
        ))}
      </section>

      <div className="operations-columns">
        <section className="panel workers-panel">
          <div className="panel__header"><div><h2>Background workers</h2><p>Durable queue and recent activity</p></div><StatusPill tone={deadJobs ? 'warning' : 'healthy'}>{deadJobs ? 'attention' : 'healthy'}</StatusPill></div>
          {error ? <p className="inline-error">{error.message}</p> : null}
          <div className="worker-row"><span className="worker-row__icon"><Icon name="spark" size={15} /></span><div><strong>Durable jobs</strong><span>embed · extract · resolve</span></div><StatusPill tone={deadJobs ? 'warning' : 'healthy'}>{deadJobs ? 'dead' : 'active'}</StatusPill><strong className="worker-row__count">{pendingJobs}</strong></div>
          <div className="worker-row"><span className="worker-row__icon"><Icon name="shield" size={15} /></span><div><strong>Sync mirror</strong><span>{sync ? `${sync.totalImportedChunks}/${sync.totalExportedChunks} chunks` : 'checking'}</span></div><StatusPill tone={sync?.unseenChunkCount ? 'warning' : 'healthy'}>{sync?.unseenChunkCount ? 'pending' : 'current'}</StatusPill><strong className="worker-row__count">{sync?.unseenChunkCount ?? '—'}</strong></div>
          <div className="worker-row"><span className="worker-row__icon"><Icon name="database" size={15} /></span><div><strong>Recent jobs</strong><span>{jobs.length ? `${jobs.length} loaded` : 'none loaded'}</span></div><StatusPill tone="neutral">read-only</StatusPill><strong className="worker-row__count">{jobs.length}</strong></div>
        </section>
        <section className="panel operations-placeholder-card">
          <div className="panel__header"><div><h2>Maintenance actions</h2><p>Reserved for explicit operator controls</p></div><Icon name="sliders" size={17} /></div>
          <div className="maintenance-list">
            <div><span><Icon name="refresh" size={15} />Reindex embeddings</span><button className="button button--quiet button--small" disabled type="button">Read-only</button></div>
            <div><span><Icon name="shield" size={15} />Verify anchors</span><button className="button button--quiet button--small" disabled type="button">Read-only</button></div>
            <div><span><Icon name="archive" size={15} />Consolidation scan</span><button className="button button--quiet button--small" disabled type="button">Read-only</button></div>
          </div>
          <p className="placeholder-disclaimer"><Icon name="info" size={14} /> Mutations stay outside this observatory surface.</p>
        </section>
      </div>
    </div>
  )
}
