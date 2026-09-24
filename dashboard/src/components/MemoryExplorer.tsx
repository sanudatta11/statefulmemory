import { useEffect, useMemo, useState, type FormEvent } from 'react'
import type { MemoryRecord, VerifyState } from '../api/types'
import { Icon } from './Icons'
import { EmptyState, ErrorState, LoadingState, StatusDot, StatusPill } from './StateViews'
import { formatDate, formatNumber } from '../utils/format'

type SearchStatus = 'idle' | 'loading' | 'success' | 'error'

interface MemoryExplorerProps {
  memories: MemoryRecord[]
  projectName: string
  loading: boolean
  error: Error | null
  onRetry: () => void
  onSelect: (memory: MemoryRecord) => void
  onSearch: (query: string) => void
  searchResults?: MemoryRecord[] | null
  searchStatus?: SearchStatus
  searchError?: Error | null
  compact?: boolean
}

const typeOptions = ['all', 'decision', 'pattern', 'fix', 'preference', 'context', 'fact'] as const

function verifyLabel(state: VerifyState): string {
  if (state === 'unanchored') return 'Unanchored'
  return state.charAt(0).toUpperCase() + state.slice(1)
}

function verifyTone(state: VerifyState): 'healthy' | 'warning' | 'neutral' {
  if (state === 'verified') return 'healthy'
  if (state === 'stale' || state === 'unprovable') return 'warning'
  if (state === 'invalidated') return 'warning'
  return 'neutral'
}

function typeTone(type: MemoryRecord['type']): 'copper' | 'green' | 'blue' | 'amber' | 'neutral' {
  if (type === 'decision') return 'copper'
  if (type === 'fix') return 'green'
  if (type === 'pattern' || type === 'fact') return 'blue'
  if (type === 'preference') return 'amber'
  return 'neutral'
}

export function MemoryExplorer({
  memories,
  projectName,
  loading,
  error,
  onRetry,
  onSelect,
  onSearch,
  searchResults,
  searchStatus = 'idle',
  searchError = null,
  compact = false,
}: MemoryExplorerProps) {
  const [query, setQuery] = useState('')
  const [type, setType] = useState<(typeof typeOptions)[number]>('all')

  useEffect(() => {
    setQuery('')
    setType('all')
  }, [projectName])

  const source = searchResults ?? memories
  const visibleMemories = useMemo(
    () => (type === 'all' ? source : source.filter((memory) => memory.type === type)),
    [source, type],
  )
  const isSearching = searchStatus === 'loading'

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    onSearch(query.trim())
  }

  return (
    <section className={`panel memory-explorer ${compact ? 'memory-explorer--compact' : ''}`}>
      <div className="panel__header memory-explorer__header">
        <div>
          <div className="eyebrow"><Icon name="memory" size={13} /> Memory store</div>
          <h2>Recent memories</h2>
          <p>Search, inspect, and trace the records your agents leave behind.</p>
        </div>
        <div className="panel__header-actions">
          <span className="result-count">
            {loading ? '—' : formatNumber(visibleMemories.length)} records
          </span>
          <button className="icon-button" aria-label="Memory table options" type="button">
            <Icon name="more" size={18} />
          </button>
        </div>
      </div>

      <form className="explorer-toolbar" onSubmit={handleSubmit}>
        <label className="search-field">
          <Icon name="search" size={16} />
          <span className="sr-only">Search memories in {projectName}</span>
          <input
            aria-label={`Search memories in ${projectName}`}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search titles, content, or anchors"
            type="search"
            value={query}
          />
          {query ? (
            <button
              aria-label="Clear search"
              className="search-field__clear"
              onClick={() => {
                setQuery('')
                onSearch('')
              }}
              type="button"
            >
              <Icon name="close" size={14} />
            </button>
          ) : null}
          <kbd>⌘ ↵</kbd>
        </label>
        <label className="filter-field">
          <Icon name="filter" size={15} />
          <span className="sr-only">Filter by memory type</span>
          <select value={type} onChange={(event) => setType(event.target.value as (typeof typeOptions)[number])}>
            {typeOptions.map((option) => (
              <option key={option} value={option}>
                {option === 'all' ? 'All types' : option}
              </option>
            ))}
          </select>
          <Icon name="chevron-down" size={13} />
        </label>
        <button className="button button--quiet explorer-toolbar__search" type="submit">
          {isSearching ? <span className="button-spinner" /> : <Icon name="search" size={15} />}
          Search
        </button>
      </form>

      {searchStatus === 'error' ? (
        <ErrorState
          error={searchError}
          onRetry={() => onSearch(query)}
          title="Search could not complete"
        />
      ) : loading || isSearching ? (
        <LoadingState label={isSearching ? 'Searching local index' : 'Loading memories'} />
      ) : error ? (
        <ErrorState error={error} onRetry={onRetry} />
      ) : visibleMemories.length === 0 ? (
        <EmptyState
          description={query || type !== 'all' ? 'Try a broader query or reset the filters.' : 'No memories have been written to this project yet.'}
          icon={query ? 'search' : 'memory'}
          title={query || type !== 'all' ? 'No matching memories' : 'Memory store is empty'}
          action={
            query || type !== 'all' ? (
              <button
                className="button button--quiet"
                onClick={() => {
                  setQuery('')
                  setType('all')
                  onSearch('')
                }}
                type="button"
              >
                Reset filters
              </button>
            ) : undefined
          }
        />
      ) : (
        <div className="table-wrap">
          <table className="memory-table">
            <thead>
              <tr>
                <th scope="col">Memory</th>
                <th scope="col">Type</th>
                <th scope="col">Freshness</th>
                <th scope="col">Anchor</th>
                <th scope="col">Updated</th>
                <th scope="col" className="align-right">Score</th>
              </tr>
            </thead>
            <tbody>
              {visibleMemories.map((memory) => (
                <tr key={memory.id}>
                  <td>
                    <button className="memory-cell" onClick={() => onSelect(memory)} type="button">
                      <span className="memory-cell__icon"><Icon name="file" size={15} /></span>
                      <span className="memory-cell__copy">
                        <strong>{memory.title}</strong>
                        <small>{memory.id} · {memory.content}</small>
                      </span>
                    </button>
                  </td>
                  <td>
                    <StatusPill tone={typeTone(memory.type)}>{memory.type}</StatusPill>
                  </td>
                  <td>
                    <span className="freshness-cell">
                      <StatusDot tone={verifyTone(memory.verifyState)} />
                      {verifyLabel(memory.verifyState)}
                    </span>
                  </td>
                  <td>
                    {memory.codeAnchor ? (
                      <span className="anchor-cell" title={memory.codeAnchor}>
                        <Icon name="link" size={13} />
                        {memory.codeAnchor.split('::')[0]}
                      </span>
                    ) : (
                      <span className="muted-cell">—</span>
                    )}
                  </td>
                  <td><span className="date-cell">{formatDate(memory.updatedAt)}</span></td>
                  <td className="align-right">
                    <span className="score-cell">{memory.score ? memory.score.toFixed(2) : '—'}</span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {!loading && !error && visibleMemories.length > 0 ? (
        <div className="panel__footer table-footer">
          <span>Showing {visibleMemories.length} of {formatNumber(memories.length)} loaded memories</span>
          <span className="table-footer__hint">Click a row to open the detail drawer</span>
        </div>
      ) : null}
    </section>
  )
}
