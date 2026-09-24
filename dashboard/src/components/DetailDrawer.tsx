import { useEffect } from 'react'
import type { MemoryRecord } from '../api/types'
import { Icon } from './Icons'
import { StatusDot, StatusPill } from './StateViews'
import { formatDate, formatTime } from '../utils/format'

interface DetailDrawerProps {
  memory: MemoryRecord | null
  onClose: () => void
}

export function DetailDrawer({ memory, onClose }: DetailDrawerProps) {
  useEffect(() => {
    if (!memory) return undefined
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [memory, onClose])

  if (!memory) return null

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
          <StatusPill tone="copper">{memory.type}</StatusPill>
          <h2>{memory.title}</h2>
          <p>{memory.content}</p>
        </div>
        <div className="detail-drawer__actions">
          <button className="button button--primary" type="button">
            <Icon name="arrow-up-right" size={15} />
            Open in editor
          </button>
          <button aria-label="Copy memory id" className="icon-button" type="button">
            <Icon name="copy" size={16} />
          </button>
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Provenance</span>
          <dl className="detail-list">
            <div><dt>Record ID</dt><dd className="mono">{memory.id}</dd></div>
            <div><dt>Project</dt><dd>{memory.projectName}</dd></div>
            <div><dt>Session</dt><dd className="mono">{memory.sessionId ?? '—'}</dd></div>
            <div><dt>Created by</dt><dd>{memory.createdBy ?? 'Unknown agent'}</dd></div>
          </dl>
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Freshness</span>
          <div className="detail-freshness">
            <StatusDot tone={memory.verifyState === 'verified' ? 'healthy' : 'warning'} />
            <div>
              <strong>{memory.verifyState === 'unanchored' ? 'Unanchored memory' : `${memory.verifyState} against repository`}</strong>
              <span>Last updated {formatDate(memory.updatedAt)} at {formatTime(memory.updatedAt)}</span>
            </div>
          </div>
        </div>

        <div className="detail-section">
          <span className="detail-section__label">Code anchor</span>
          {memory.codeAnchor ? (
            <div className="anchor-detail">
              <Icon name="link" size={15} />
              <span className="mono">{memory.codeAnchor}</span>
              <button aria-label="Copy code anchor" className="icon-button icon-button--small" type="button">
                <Icon name="copy" size={14} />
              </button>
            </div>
          ) : (
            <p className="detail-muted">No repository anchor attached. This record will remain unanchored.</p>
          )}
        </div>

        <div className="detail-section detail-section--placeholder">
          <div className="placeholder-heading">
            <span className="placeholder-icon"><Icon name="archive" size={15} /></span>
            <div><strong>Record timeline</strong><span>Coming next</span></div>
          </div>
          <p>Revision history, supersession links, and verification events will live here.</p>
        </div>
      </aside>
    </>
  )
}
