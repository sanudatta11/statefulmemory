import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createDashboardApi } from './api'
import type { DashboardApi, MemoryRecord } from './api/types'
import { AppShell, type DashboardView } from './components/AppShell'
import { DetailDrawer } from './components/DetailDrawer'
import { GraphPlaceholder } from './components/GraphPlaceholder'
import { MemoryView } from './components/MemoryView'
import { OperationsPlaceholder } from './components/OperationsPlaceholder'
import { OverviewView } from './components/OverviewView'
import { RetrievalLab } from './components/RetrievalLab'
import { ErrorState } from './components/StateViews'
import { useDashboardData } from './hooks/useDashboardData'
import { useProjectState } from './hooks/useProjectState'

interface AppProps {
  api?: DashboardApi
  initialView?: DashboardView
}

function toError(value: unknown): Error {
  return value instanceof Error ? value : new Error('Search request failed')
}

export function App({ api: injectedApi, initialView = 'overview' }: AppProps) {
  const defaultApi = useMemo(() => createDashboardApi(), [])
  const api = injectedApi ?? defaultApi
  const { project, selectProject } = useProjectState()
  const { data, projects, status, error, refresh } = useDashboardData(api, project)
  const [activeView, setActiveView] = useState<DashboardView>(initialView)
  const [selectedMemory, setSelectedMemory] = useState<MemoryRecord | null>(null)
  const [searchResults, setSearchResults] = useState<MemoryRecord[] | null>(null)
  const [searchStatus, setSearchStatus] = useState<'idle' | 'loading' | 'success' | 'error'>('idle')
  const [searchError, setSearchError] = useState<Error | null>(null)
  const searchController = useRef<AbortController | null>(null)
  const mode = import.meta.env.VITE_DASHBOARD_DATA_SOURCE === 'api' ? 'api' : 'preview'

  useEffect(() => {
    searchController.current?.abort()
    searchController.current = null
    setSearchResults(null)
    setSearchStatus('idle')
    setSearchError(null)
    setSelectedMemory(null)
  }, [project])

  const handleSearch = useCallback(
    (query: string) => {
      searchController.current?.abort()
      const normalizedQuery = query.trim()
      if (!normalizedQuery) {
        setSearchResults(null)
        setSearchStatus('idle')
        setSearchError(null)
        return
      }
      const controller = new AbortController()
      searchController.current = controller
      setSearchStatus('loading')
      setSearchError(null)
      void api.searchMemories(project, normalizedQuery, { signal: controller.signal }).then(
        (response) => {
          if (controller.signal.aborted) return
          setSearchResults(response.memories)
          setSearchStatus('success')
        },
        (reason: unknown) => {
          if (controller.signal.aborted) return
          setSearchError(toError(reason))
          setSearchStatus('error')
        },
      )
    },
    [api, project],
  )

  const handleProjectChange = useCallback(
    (nextProject: string) => {
      selectProject(nextProject)
      setActiveView('overview')
    },
    [selectProject],
  )

  const handleRefresh = useCallback(() => {
    void refresh()
  }, [refresh])

  const commonExplorerProps = {
    projectName: project,
    memories: data?.overview ? data.memories : [],
    loading: status === 'loading',
    error: status === 'error' ? error : null,
    onRetry: handleRefresh,
    onSelect: setSelectedMemory,
    onSelectMemory: setSelectedMemory,
    onSearch: handleSearch,
    searchResults,
    searchStatus,
    searchError,
  }

  let content
  if (status === 'error') {
    content = (
      <div className="page-stack">
        <div className="page-heading">
          <div>
            <div className="eyebrow">Local project workspace</div>
            <h1>Workspace unavailable</h1>
            <p>The dashboard could not reach the local service for {project}.</p>
          </div>
        </div>
        <ErrorState error={error} onRetry={handleRefresh} title="Could not load project memory" />
      </div>
    )
  } else {
    switch (activeView) {
      case 'memory':
        content = <MemoryView {...commonExplorerProps} />
        break
      case 'graph':
        content = <GraphPlaceholder projectName={project} api={api} />
        break
      case 'retrieval':
        content = <RetrievalLab projectName={project} api={api} />
        break
      case 'operations':
        content = <OperationsPlaceholder projectName={project} api={api} health={data?.overview.health} onRefresh={handleRefresh} />
        break
      case 'overview':
      default:
        content = (
          <OverviewView
            {...commonExplorerProps}
            health={data?.overview.health}
            onOpenGraph={() => setActiveView('graph')}
            onOpenMemory={() => setActiveView('memory')}
            onOpenRetrieval={() => setActiveView('retrieval')}
            stats={data?.overview.stats}
          />
        )
        break
    }
  }

  return (
    <AppShell
      activeView={activeView}
      connectionState={data?.overview.health.state ?? 'degraded'}
      mode={mode}
      onNavigate={setActiveView}
      onProjectChange={handleProjectChange}
      onRefresh={handleRefresh}
      project={project}
      projects={projects}
    >
      {content}
      <DetailDrawer memory={selectedMemory} onClose={() => setSelectedMemory(null)} />
    </AppShell>
  )
}
