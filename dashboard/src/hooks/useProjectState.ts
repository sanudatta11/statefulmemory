import { useCallback, useEffect, useState } from 'react'

export const DEFAULT_PROJECT = 'memlayer'

function normalizeProject(value: string | null | undefined): string {
  const normalized = value?.trim()
  return normalized ? normalized.slice(0, 120) : DEFAULT_PROJECT
}

export function readProjectFromUrl(): string {
  if (typeof window === 'undefined') {
    return DEFAULT_PROJECT
  }
  return normalizeProject(new URLSearchParams(window.location.search).get('project'))
}

function writeProjectToUrl(projectName: string, replace = false): void {
  if (typeof window === 'undefined') {
    return
  }
  const query = new URLSearchParams(window.location.search)
  query.set('project', projectName)
  const nextUrl = `${window.location.pathname}?${query.toString()}${window.location.hash}`
  if (replace) {
    window.history.replaceState({}, '', nextUrl)
  } else {
    window.history.pushState({}, '', nextUrl)
  }
}

export function useProjectState() {
  const [project, setProject] = useState(readProjectFromUrl)
  const [isReady, setIsReady] = useState(false)

  useEffect(() => {
    const currentProject = readProjectFromUrl()
    setProject(currentProject)
    if (!new URLSearchParams(window.location.search).has('project')) {
      writeProjectToUrl(currentProject, true)
    }
    const handlePopState = () => setProject(readProjectFromUrl())
    window.addEventListener('popstate', handlePopState)
    setIsReady(true)
    return () => window.removeEventListener('popstate', handlePopState)
  }, [])

  const selectProject = useCallback((nextProject: string) => {
    const normalized = normalizeProject(nextProject)
    setProject(normalized)
    writeProjectToUrl(normalized)
  }, [])

  return { project, selectProject, isReady }
}
