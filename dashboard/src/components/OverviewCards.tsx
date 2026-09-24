import type { DashboardStats } from '../api/types'
import { Icon, type IconName } from './Icons'
import { SkeletonBlock } from './StateViews'
import { formatCompact, formatNumber, formatRelativeTime } from '../utils/format'

interface OverviewCardsProps {
  stats: DashboardStats | undefined
  loading: boolean
}

interface CardDefinition {
  label: string
  value?: string
  detail: string
  icon: IconName
  tone: 'copper' | 'green' | 'blue' | 'amber'
  trend?: string
}

function getDefinitions(stats?: DashboardStats): CardDefinition[] {
  return [
    {
      label: 'Memory store',
      value: stats ? formatNumber(stats.totalMemories) : undefined,
      detail: stats ? `+${formatNumber(stats.addedThisWeek)} this week` : 'Loading local records',
      icon: 'database',
      tone: 'copper',
      trend: stats ? `+${Math.round((stats.addedThisWeek / Math.max(stats.totalMemories, 1)) * 1000) / 10}%` : undefined,
    },
    {
      label: 'Freshness',
      value: stats ? `${stats.verifiedPercent.toFixed(1)}%` : undefined,
      detail: stats ? `${stats.indexCoveragePercent.toFixed(1)}% index coverage` : 'Checking anchors',
      icon: 'shield',
      tone: 'green',
      trend: stats ? 'healthy' : undefined,
    },
    {
      label: 'Open conflicts',
      value: stats ? formatNumber(stats.openConflicts) : undefined,
      detail: stats && stats.openConflicts > 0 ? 'Needs review' : 'Nothing waiting',
      icon: 'warning',
      tone: stats && stats.openConflicts > 0 ? 'amber' : 'green',
      trend: stats && stats.openConflicts > 0 ? 'review' : undefined,
    },
    {
      label: 'Entity graph',
      value: stats ? formatCompact(stats.indexedEntities) : undefined,
      detail: stats ? `Indexed ${formatCompact(stats.indexedEntities)} nodes` : 'Loading entities',
      icon: 'graph',
      tone: 'blue',
      trend: stats ? 'PPR ready' : undefined,
    },
  ]
}

export function OverviewCards({ stats, loading }: OverviewCardsProps) {
  return (
    <section aria-label="Project overview" className="overview-grid">
      {getDefinitions(stats).map((card) => (
        <article className={`stat-card stat-card--${card.tone}`} key={card.label}>
          <div className="stat-card__topline">
            <span className="stat-card__icon">
              <Icon name={card.icon} size={17} />
            </span>
            {card.trend ? <span className="stat-card__trend">{card.trend}</span> : null}
          </div>
          <div className="stat-card__label">{card.label}</div>
          {loading || !card.value ? <SkeletonBlock className="skeleton-block--number" /> : <div className="stat-card__value">{card.value}</div>}
          <div className="stat-card__detail">
            {loading ? <SkeletonBlock className="skeleton-block--line" /> : card.detail}
          </div>
          {stats ? <div className="stat-card__footer">Updated {formatRelativeTime(stats.lastIndexedAt)}</div> : null}
        </article>
      ))}
    </section>
  )
}
