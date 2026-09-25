import { useEffect } from 'react'
import type { GraphEntity, MemoryRecord, ObservationDetail } from '../api/types'
import { Icon } from './Icons'
import { StatusDot, StatusPill } from './StateViews'
import { formatDate, formatTime } from '../utils/format'

interface DetailDrawerProps {
  memory: MemoryRecord | null
  detail: ObservationDetail | null
  detailStatus: 'idle' | 'loading' | 'success' | 'error'
  detailError: Error | null
  onClose: () => void
}

function text(value: unknown, fallback = '—'): string {
  if (typeof value === 'string' && value.trim()) return value
  if (typeof value === 'number') return String(value)
  return fallback
}

function factLine(value: Record<string, unknown>): string {
  return [text(value.subject), text(value.predicate), text(value.object)].filter((item) => item !== '—').join(' · ')
}

function relationLine(value: Record<string, unknown>): string {
  return `${text(value.relation_type)} · ${text(value.source_id)} → ${text(value.target_id)}`
}

function entityLine(value: Pick<GraphEntity, 'kind' | 'name'>): string {
  return `${text(value.kind)} · ${text(value.name)}`
}

export function DetailDrawer({ memory, detail, detailStatus, detailError, onClose }: DetailDrawerProps) {
  useEffect(() => {
    if (!memory) return undefined
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [memory, onClose])

  if (!memory) return null
  const observation = detail?.observation ?? memory
  const anchors = detail?.anchors ?? []
  const facts = detail?.facts ?? []
  const relations = detail?.relations ?? []
  const entities = detail?.entities ?? []
  const history = detail?.history ?? []

  return (
    <>
      <button aria-label="Close memory details" className="drawer-backdrop" onClick={onClose} type="button" />
      <aside aria-label="Memory details" aria-modal="true" className="detail-drawer" role="dialog">
        <div className="detail-drawer__topline">
          <span className="eyebrow"><Icon name="file" size={13} /> Memory record</span>
          <button aria-label="Close memory details" className="icon-button" onClick={onClose} type="button">
            <Icon name="close" size={18} />
          </button>
        </div>
        <div className="detail-drawer__title-block">
          <StatusPill tone="copper">{observation.type}</StatusPill>
          <h2>{observation.title}</h2>
          <p>{observation.content}</p>
        </div>
        {detailStatus === 'loading' ? <div className="loading-state"><span className="loading-state__mark"><i /><i /><i /></span>Loading provenance…</div> : null}
        {detailStatus === 'error' ? <p className="detail-error">{detailError?.message ?? 'Detail lookup failed'}</p> : null}
        <div className="detail-drawer__actions">
          <button
            className="button button--primary"
            onClick={() => void navigator.clipboard?.writeText(observation.id)}
            type="button"
          >
            <Icon name="copy" size={15} />
            Copy record ID
          </button>
          <span className="mono detail-record-id">{observation.id}</span>
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Provenance</span>
          <dl className="detail-list">
            <div><dt>Record ID</dt><dd className="mono">{observation.id}</dd></div>
            <div><dt>Project</dt><dd>{observation.projectName}</dd></div>
            <div><dt>Session</dt><dd className="mono">{observation.sessionId ?? '—'}</dd></div>
            <div><dt>Created by</dt><dd>{observation.createdBy ?? 'Unknown agent'}</dd></div>
            <div><dt>Revisions</dt><dd>{observation.revisionCount}</dd></div>
            <div><dt>Superseded</dt><dd>{observation.supersededCount}</dd></div>
          </dl>
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Freshness</span>
          <div className="detail-freshness">
            <StatusDot tone={observation.verifyState === 'verified' ? 'healthy' : 'warning'} />
            <div>
              <strong>{observation.verifyState === 'unanchored' ? 'Unanchored memory' : `${observation.verifyState} against repository`}</strong>
              <span>Last updated {formatDate(observation.updatedAt)} at {formatTime(observation.updatedAt)}</span>
            </div>
          </div>
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Code anchors</span>
          {anchors.length ? anchors.map((anchor, index) => (
            <div className="anchor-detail" key={`${text(anchor.path)}-${index}`}>
              <Icon name="link" size={15} />
              <span className="mono">{text(anchor.path)}{text(anchor.symbol) === '—' ? '' : `::${text(anchor.symbol)}`}</span>
            </div>
          )) : observation.codeAnchor ? (
            <div className="anchor-detail"><Icon name="link" size={15} /><span className="mono">{observation.codeAnchor}</span></div>
          ) : (
            <p className="detail-muted">No repository anchor attached. This record remains unanchored.</p>
          )}
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Atomic facts</span>
          {facts.length ? <div className="detail-chip-list">{facts.map((fact, index) => <span className="detail-chip" key={`${factLine(fact)}-${index}`}>{factLine(fact)}</span>)}</div> : <p className="detail-muted">No extracted facts attached.</p>}
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Relations and entities</span>
          {relations.length || entities.length ? (
            <div className="detail-chip-list">
              {relations.map((relation, index) => <span className="detail-chip detail-chip--relation" key={`${relationLine(relation)}-${index}`}>{relationLine(relation)}</span>)}
              {entities.map((entity, index) => <span className="detail-chip detail-chip--entity" key={`${entity.name}-${index}`}>{entityLine(entity)}</span>)}
            </div>
          ) : <p className="detail-muted">No graph links attached.</p>}
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Revision history</span>
          {history.length ? (
            <div className="history-list">
              {history.map((entry) => (
                <div className="history-entry" key={entry.id}>
                  <span className="history-entry__dot" />
                  <div><strong>{entry.title}</strong><span>#{entry.id} · {formatDate(entry.createdAt)}</span></div>
                </div>
              ))}
            </div>
          ) : <p className="detail-muted">No supersession history recorded.</p>}
        </div>
      </aside>
    </>
  )
}
