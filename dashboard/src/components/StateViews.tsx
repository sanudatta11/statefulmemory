import type { ReactNode } from 'react'
import { Icon, type IconName } from './Icons'

export function LoadingState({ label = 'Loading local index' }: { label?: string }) {
  return (
    <div className="loading-state" role="status" aria-live="polite">
      <span className="loading-state__mark" aria-hidden="true">
        <span />
        <span />
        <span />
      </span>
      <span>{label}</span>
    </div>
  )
}

export function ErrorState({
  title = 'Could not load this view',
  error,
  onRetry,
}: {
  title?: string
  error?: Error | null
  onRetry?: () => void
}) {
  return (
    <div className="error-state" role="alert">
      <span className="state-icon state-icon--error">
        <Icon name="warning" size={18} />
      </span>
      <div className="state-copy">
        <strong>{title}</strong>
        <span>{error?.message ?? 'The local service did not return a response.'}</span>
      </div>
      {onRetry ? (
        <button className="button button--quiet" onClick={onRetry} type="button">
          <Icon name="refresh" size={15} />
          Retry
        </button>
      ) : null}
    </div>
  )
}

export function EmptyState({
  title,
  description,
  icon = 'archive',
  action,
}: {
  title: string
  description: string
  icon?: IconName
  action?: ReactNode
}) {
  return (
    <div className="empty-state">
      <span className="state-icon">
        <Icon name={icon} size={19} />
      </span>
      <strong>{title}</strong>
      <span>{description}</span>
      {action}
    </div>
  )
}

export function StatusDot({ tone = 'healthy' }: { tone?: 'healthy' | 'warning' | 'offline' | 'neutral' }) {
  return <span aria-hidden="true" className={`status-dot status-dot--${tone}`} />
}

export function StatusPill({
  children,
  tone = 'neutral',
  icon,
}: {
  children: ReactNode
  tone?: 'healthy' | 'warning' | 'danger' | 'neutral' | 'copper' | 'blue' | 'green' | 'amber'
  icon?: IconName
}) {
  return (
    <span className={`status-pill status-pill--${tone}`}>
      {icon ? <Icon name={icon} size={13} /> : null}
      {children}
    </span>
  )
}

export function SkeletonBlock({ className = '' }: { className?: string }) {
  return <span aria-hidden="true" className={`skeleton-block ${className}`} />
}
