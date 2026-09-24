import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { readProjectFromUrl, useProjectState } from './useProjectState'

describe('useProjectState', () => {
  beforeEach(() => {
    window.history.replaceState({}, '', '/')
  })

  it('reads the project from the URL and normalizes absent state', () => {
    expect(readProjectFromUrl()).toBe('memlayer')
    window.history.replaceState({}, '', '/?project=atlas-api')
    expect(readProjectFromUrl()).toBe('atlas-api')
  })

  it('writes project changes to the URL and responds to browser navigation', () => {
    const { result } = renderHook(() => useProjectState())

    act(() => result.current.selectProject('laya-lab'))
    expect(result.current.project).toBe('laya-lab')
    expect(new URLSearchParams(window.location.search).get('project')).toBe('laya-lab')

    act(() => {
      window.history.pushState({}, '', '/?project=atlas-api')
      window.dispatchEvent(new PopStateEvent('popstate'))
    })
    expect(result.current.project).toBe('atlas-api')
  })
})
