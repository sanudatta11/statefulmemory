import { describe, expect, it, vi } from 'vitest'
import { DashboardApiError, HttpDashboardApi } from './http-client'

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('HttpDashboardApi', () => {
  it('encodes project names and normalizes memory responses', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
      jsonResponse({ observations: [{ id: 'mem-1' }], next_cursor: 'next' }),
    )
    const api = new HttpDashboardApi('', fetcher)

    const result = await api.listMemories('team/one')

    expect(result).toMatchObject({
      memories: [{ id: 'mem-1', projectName: 'team/one' }],
      nextCursor: 'next',
    })
    expect(fetcher).toHaveBeenCalledWith(
      '/api/projects/team%2Fone/memories?limit=50',
      expect.objectContaining({ method: 'GET' }),
    )
  })

  it('maps the loopback stats response into dashboard cards', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
      jsonResponse({ observations_scanned: 42, consolidation_proposals: 3 }),
    )
    const api = new HttpDashboardApi('', fetcher)

    const overview = await api.getProjectOverview('memlayer')

    expect(overview.stats.totalMemories).toBe(42)
    expect(overview.stats.openConflicts).toBe(3)
    expect(fetcher).toHaveBeenCalledWith(
      '/api/stats',
      expect.objectContaining({
        method: 'POST',
        body: '{}',
        credentials: 'include',
      }),
    )
  })

  it('sends the strict search contract used by the local UI', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(jsonResponse({ hits: [] }))
    const api = new HttpDashboardApi('', fetcher)

    await api.searchMemories('memlayer', 'write thread')

    expect(fetcher).toHaveBeenCalledWith(
      '/api/search',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ query: 'write thread', limit: 15 }),
      }),
    )
  })

  it('normalizes snake-case search hits into explorer records', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
      jsonResponse({
        hits: [
          {
            id: 17,
            type: 'decision',
            title: 'Write thread',
            content: 'Keep writes serialized.',
            created_at: '2026-09-25T08:00:00.000Z',
            verify_state: 'verified',
            code_anchor: 'storage/write.rs::write',
          },
        ],
      }),
    )
    const api = new HttpDashboardApi('', fetcher)

    const result = await api.searchMemories('memlayer', 'write')

    expect(result.memories[0]).toMatchObject({
      id: '17',
      projectName: 'memlayer',
      verifyState: 'verified',
      codeAnchor: 'storage/write.rs::write',
      revisionCount: 0,
    })
  })

  it('surfaces API error messages with status context', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(jsonResponse({ error: 'daemon unavailable' }, 503))
    const api = new HttpDashboardApi('', fetcher)

    await expect(api.listProjects()).rejects.toMatchObject({
      name: 'DashboardApiError',
      message: 'daemon unavailable',
      status: 503,
      code: 'http_503',
      retryable: true,
    })
  })

  it('wraps network failures in a retryable API error', async () => {
    const fetcher = vi.fn<typeof fetch>().mockRejectedValue(new TypeError('connection refused'))
    const api = new HttpDashboardApi('', fetcher)

    const error = await api.listProjects().catch((reason: unknown) => reason)

    expect(error).toBeInstanceOf(DashboardApiError)
    expect(error).toMatchObject({ code: 'network_error', retryable: true, status: 0 })
  })

  it('rejects malformed success payloads', async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('{not-json', { status: 200 }))
    const api = new HttpDashboardApi('', fetcher)

    await expect(api.listProjects()).rejects.toMatchObject({
      name: 'DashboardApiError',
      code: 'invalid_json',
      status: 200,
    })
  })
})
