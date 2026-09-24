import type { MemoryRecord } from '../api/types'
import { Icon } from './Icons'
import { MemoryExplorer } from './MemoryExplorer'

interface MemoryViewProps {
  projectName: string
  memories: MemoryRecord[]
  loading: boolean
  error: Error | null
  onRetry: () => void
  onSelect: (memory: MemoryRecord) => void
  onSearch: (query: string) => void
  searchResults: MemoryRecord[] | null
  searchStatus: 'idle' | 'loading' | 'success' | 'error'
  searchError: Error | null
}

export function MemoryView({
  projectName,
  memories,
  loading,
  error,
  onRetry,
  onSelect,
  onSearch,
  searchResults,
  searchStatus,
  searchError,
}: MemoryViewProps) {
  return (
    <div className="page-stack">
      <div className="page-heading">
        <div>
          <div className="eyebrow"><Icon name="memory" size={13} /> Full workspace view</div>
          <h1>Memory explorer</h1>
          <p>Every decision, pattern, and correction in {projectName}—searchable without leaving localhost.</p>
        </div>
        <button className="button button--primary" type="button"><Icon name="plus" size={16} />Add memory</button>
      </div>
      <MemoryExplorer
        error={error}
        loading={loading}
        memories={memories}
        onRetry={onRetry}
        onSearch={onSearch}
        onSelect={onSelect}
        projectName={projectName}
        searchError={searchError}
        searchResults={searchResults}
        searchStatus={searchStatus}
      />
    </div>
  )
}
