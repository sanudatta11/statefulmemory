import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  DashboardApi,
  MemoryRecord,
  ProjectOverview,
  ProjectSummary,
} from '../api/types'

export type LoadStatus = 'loading' | 'success' | 'error'

interface DashboardData {
  overview: ProjectOverview
  memories: MemoryRecord[]
}

interface DashboardState {
  data: DashboardData | null
  projects: ProjectSummary[]
  status: LoadStatus
  error: Error | null
  isRefreshing: boolean
}

function toError(error: unknown): Error {
  return error instanceof Error ? error : new Error('Unknown dashboard error')
}

export function useDashboardData(api: DashboardApi, projectName: string) {
  const [state, setState] = useState<DashboardState>({
    data: null,
    projects: [],
    status: 'loading',
    error: null,
    isRefreshing: false,
  })
  const requestController = useRef<AbortController | null>(null)

  const refresh = useCallback(async () => {
    requestController.current?.abort()
    const controller = new AbortController()
    requestController.current = controller
    setState((current) => ({
      ...current,
      status: 'loading',
      error: null,
      isRefreshing: current.data !== null,
    }))

    const [overviewResult, memoriesResult, projectsResult] = await Promise.allSettled([
      api.getProjectOverview(projectName, { signal: controller.signal }),
      api.listMemories(projectName, { signal: controller.signal }),
      api.listProjects({ signal: controller.signal }),
    ])

    if (controller.signal.aborted || requestController.current !== controller) {
      return
    }

    if (overviewResult.status === 'rejected') {
      setState((current) => ({
        ...current,
        status: 'error',
        error: toError(overviewResult.reason),
        isRefreshing: false,
      }))
      return
    }

    if (memoriesResult.status === 'rejected') {
      setState((current) => ({
        ...current,
        status: 'error',
        error: toError(memoriesResult.reason),
        isRefreshing: false,
      }))
      return
    }

    const projects =
      projectsResult.status === 'fulfilled' && projectsResult.value.length > 0
        ? projectsResult.value
        : [overviewResult.value.project]
    setState({
      data: {
        overview: overviewResult.value,
        memories: memoriesResult.value.memories,
      },
      projects,
      status: 'success',
      error: null,
      isRefreshing: false,
    })
  }, [api, projectName])

  useEffect(() => {
    void refresh()
    return () => {
      requestController.current?.abort()
    }
  }, [refresh])

  return { ...state, refresh }
}
