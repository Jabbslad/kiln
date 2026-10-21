import { describe, expect, it, vi } from 'vitest'
import {
  createBox,
  dashboardSchema,
  fetchDashboard,
  formatBytes,
  formatMetric,
  filterBoxes,
} from './data'
import { previewDashboard } from './fixtures'

describe('dashboard boundary', () => {
  it('validates preview fixtures against the same contract as the backend', () => {
    expect(dashboardSchema.parse(previewDashboard).boxes).toHaveLength(4)
  })

  it('rejects missing state instead of guessing running', () => {
    const value = structuredClone(previewDashboard)
    Reflect.deleteProperty(value.boxes[0], 'state')
    expect(() => dashboardSchema.parse(value)).toThrow()
  })

  it('rejects an incompatible schema version', () => {
    expect(() => dashboardSchema.parse({ ...previewDashboard, schema_version: 2 })).toThrow()
  })

  it('does not render missing telemetry as zero', () => {
    expect(formatMetric(null, '%')).toBe('—')
    expect(formatMetric(0, '%')).toBe('0%')
    expect(formatMetric(17.3, '%')).toBe('17.3%')
    expect(formatBytes(null)).toBe('—')
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1536 * 1024 * 1024)).toBe('1.5 GiB')
  })

  it('combines case-insensitive name/id search with state filters', () => {
    expect(filterBoxes(previewDashboard.boxes, 'WORKER', 'paused').map((box) => box.name)).toEqual([
      'worker-test',
    ])
    expect(filterBoxes(previewDashboard.boxes, 'WORKER', 'running')).toEqual([])
    expect(filterBoxes(previewDashboard.boxes, '7f3a', 'all').map((box) => box.name)).toEqual([
      'api-dev',
    ])
  })

  it('reports service failure without an empty-inventory fallback', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response('unavailable', { status: 503 }))
    await expect(fetchDashboard(undefined, fetcher)).rejects.toThrow('503')
    expect(fetcher).toHaveBeenCalledTimes(1)
  })

  it('rejects the SPA HTML fallback rather than presenting a connected empty workspace', async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(
        new Response('<!doctype html>', { headers: { 'Content-Type': 'text/html' } }),
      )
    await expect(fetchDashboard(undefined, fetcher)).rejects.toThrow('dashboard API')
  })

  it('validates a successful JSON response and uses same-origin credentials', async () => {
    const fetcher = vi.fn().mockResolvedValue(Response.json(previewDashboard))
    const controller = new AbortController()
    expect(await fetchDashboard(controller.signal, fetcher)).toEqual(previewDashboard)
    expect(fetcher).toHaveBeenCalledWith(
      '/api/v1/dashboard',
      expect.objectContaining({ signal: controller.signal, credentials: 'same-origin' }),
    )
  })

  it('posts one create and withholds the box until the dashboard observes it', async () => {
    const boxId = 'b'.repeat(32)
    const seen = structuredClone(previewDashboard)
    seen.boxes = [
      {
        ...previewDashboard.boxes[0],
        id: boxId,
        name: 'api-staging',
      },
    ]
    const calls: Array<{ url: string; init?: RequestInit }> = []
    let observed = false
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      calls.push({ url, init })
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        expect(posted.action).toEqual({ type: 'create', template: 'ubuntu', name: 'api-staging' })
        expect(posted.id).toMatch(/^[a-f0-9]{32}$/)
        return Response.json(
          { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
          { status: 202 },
        )
      }
      if (url.startsWith('/v1/operations/')) {
        const id = url.slice('/v1/operations/'.length)
        return Response.json({ id, box_id: boxId, state: 'succeeded', result: null, error: null })
      }
      if (!observed) {
        observed = true
        return Response.json({ ...previewDashboard, boxes: [] })
      }
      return Response.json(seen)
    })
    const dashboard = await createBox(
      'ubuntu',
      'api-staging',
      fetcher,
      '__Host-kiln_csrf=form-csrf',
    )
    expect(dashboard.boxes.map((box) => box.id)).toEqual([boxId])
    const posts = calls.filter((call) => call.url === '/v1/operations')
    expect(posts).toHaveLength(1)
    expect(posts[0].init?.headers).toMatchObject({ 'x-kiln-csrf': 'form-csrf' })
    expect(posts[0].init?.credentials).toBe('same-origin')
  })
})
