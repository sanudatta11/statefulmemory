import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { MemoryExplorer } from './MemoryExplorer'
import type { MemoryRecord } from '../api/types'

const memory: MemoryRecord = {
  id: 'mem-1',
  projectName: 'test-project',
  title: 'A remembered decision',
  content: 'Keep this in the local index.',
  type: 'decision',
  scope: 'project',
  createdAt: '2026-09-25T08:00:00.000Z',
  updatedAt: '2026-09-25T08:00:00.000Z',
  verifyState: 'verified',
  revisionCount: 1,
  supersededCount: 0,
}

function renderExplorer(overrides: Partial<React.ComponentProps<typeof MemoryExplorer>> = {}) {
  return render(
    <MemoryExplorer
      error={null}
      loading={false}
      memories={[memory]}
      onRetry={() => undefined}
      onSearch={() => undefined}
      onSelect={() => undefined}
      projectName="test-project"
      {...overrides}
    />,
  )
}

describe('MemoryExplorer states', () => {
  it('announces loading state', () => {
    renderExplorer({ loading: true, memories: [] })
    expect(screen.getByRole('status')).toHaveTextContent('Loading memories')
  })

  it('shows an empty state when no records exist', () => {
    renderExplorer({ memories: [] })
    expect(screen.getByText('Memory store is empty')).toBeInTheDocument()
  })

  it('shows a retryable error state', () => {
    renderExplorer({ error: new Error('daemon unavailable') })
    expect(screen.getByRole('alert')).toHaveTextContent('daemon unavailable')
    expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument()
  })
})
