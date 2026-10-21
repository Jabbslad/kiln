import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  applyLifecycle,
  createBox,
  createCheckpoint,
  dashboardSchema,
  deleteBox,
  execCommand,
  fetchDashboard,
  formatBytes,
  formatDiskUsedOfTotal,
  formatMetric,
  filterBoxes,
  restoreCheckpoint,
  submitOperation,
  trackOperation,
  type Dashboard,
  type OperationAction,
  type OperationActionType,
} from './data'
import { previewDashboard } from './fixtures'

function slowRunningFetcher(
  boxId: string,
  pollsBeforeSuccess: number,
  succeeded: Record<string, unknown>,
  dashboardDone: Dashboard,
  dashboardPending: Dashboard = previewDashboard,
) {
  let pollCount = 0
  let finished = false
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    if (url === '/v1/operations' && init?.method === 'POST') {
      const posted = JSON.parse(String(init.body))
      return Response.json(
        { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
        { status: 202 },
      )
    }
    if (url.startsWith('/v1/operations/')) {
      pollCount += 1
      const id = url.slice('/v1/operations/'.length)
      if (pollCount < pollsBeforeSuccess) {
        return Response.json({ id, box_id: boxId, state: 'running', result: null, error: null })
      }
      finished = true
      return Response.json({ id, box_id: boxId, ...succeeded, error: null })
    }
    return Response.json(finished ? dashboardDone : dashboardPending)
  })
}

afterEach(() => {
  vi.useRealTimers()
})

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
    expect(formatDiskUsedOfTotal(858_993_459, 2 * 1024 ** 3)).toBe('0.8 of 2.0 GiB')
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

  it('keeps polling a create that stays running longer than twenty instant polls', async () => {
    const boxId = 'b'.repeat(32)
    const seen = structuredClone(previewDashboard)
    seen.boxes = [{ ...previewDashboard.boxes[0], id: boxId, name: 'slow-boot' }]
    let pollCount = 0
    let observed = false
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json(
          { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
          { status: 202 },
        )
      }
      if (url.startsWith('/v1/operations/')) {
        pollCount += 1
        const id = url.slice('/v1/operations/'.length)
        if (pollCount < 8) {
          return Response.json({ id, box_id: boxId, state: 'running', result: null, error: null })
        }
        return Response.json({ id, box_id: boxId, state: 'succeeded', result: null, error: null })
      }
      if (!observed) {
        observed = true
        return Response.json({ ...previewDashboard, boxes: [] })
      }
      return Response.json(seen)
    })
    const dashboard = await createBox('ubuntu', 'slow-boot', fetcher, '__Host-kiln_csrf=form-csrf')
    expect(pollCount).toBeGreaterThanOrEqual(8)
    expect(dashboard.boxes.map((box) => box.id)).toEqual([boxId])
  })

  it('reports a create still running after the wait budget expires', async () => {
    const boxId = 'c'.repeat(32)
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json(
          { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
          { status: 202 },
        )
      }
      if (url.startsWith('/v1/operations/')) {
        const id = url.slice('/v1/operations/'.length)
        return Response.json({ id, box_id: boxId, state: 'running', result: null, error: null })
      }
      return Response.json(previewDashboard)
    })
    await expect(
      trackOperation(
        { type: 'create', template: 'ubuntu', name: 'never-finishes' },
        fetcher,
        '__Host-kiln_csrf=form-csrf',
        'd'.repeat(32),
        120,
      ),
    ).rejects.toThrow(/still in progress/i)
  })

  it('surfaces the host error when create returns failed', async () => {
    const boxId = 'd'.repeat(32)
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json(
          {
            id: posted.id,
            box_id: boxId,
            state: 'failed',
            result: null,
            error: { message: 'No free box slot on this host.' },
          },
          { status: 202 },
        )
      }
      return Response.json(previewDashboard)
    })
    await expect(
      createBox('ubuntu', 'jamie', fetcher, '__Host-kiln_csrf=form-csrf'),
    ).rejects.toThrow('No free box slot on this host.')
  })

  it('treats unknown create receipts as still in progress, not failure', async () => {
    const boxId = 'e'.repeat(32)
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json(
          {
            id: posted.id,
            box_id: boxId,
            state: 'unknown',
            result: null,
            error: { message: 'Operation did not return a confirmed result.' },
          },
          { status: 202 },
        )
      }
      return Response.json(previewDashboard)
    })
    await expect(
      createBox('ubuntu', 'jamie', fetcher, '__Host-kiln_csrf=form-csrf'),
    ).rejects.toThrow(/still in progress/i)
  })

  it('rejects a checkpoint when CSRF, Origin, or the session fails', async () => {
    const cases = [
      {
        status: 403,
        body: { code: 'invalid_csrf', message: 'The request was refused.' },
        message: /csrf/i,
      },
      {
        status: 403,
        body: { code: 'invalid_origin', message: 'The request was refused.' },
        message: /origin/i,
      },
      {
        status: 401,
        body: { code: 'unauthorized', message: 'A valid credential is required.' },
        message: /401/,
      },
    ]
    for (const item of cases) {
      const fetcher = vi.fn(async () => Response.json(item.body, { status: item.status }))
      await expect(
        submitOperation(
          { type: 'checkpoint', id: 'a'.repeat(32) },
          fetcher,
          '__Host-kiln_csrf=form-csrf',
        ),
      ).rejects.toThrow(item.message)
      expect(fetcher).toHaveBeenCalledTimes(1)
    }
  })

  it('retries one checkpoint id and withholds it until the dashboard lists that box', async () => {
    const request = 'a'.repeat(32)
    const checkpoint = 'b'.repeat(32)
    const box = 'c'.repeat(32)
    const seen = structuredClone(previewDashboard)
    seen.checkpoints = [
      {
        id: checkpoint,
        box_id: box,
        name: 'shift-snapshot',
        created_at: previewDashboard.observed_at,
        size_bytes: 4096,
        compatible: true,
      },
    ]
    let posts = 0
    let reads = 0
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        posts += 1
        const posted = JSON.parse(String(init.body))
        expect(posted.id).toBe(request)
        expect(posted.action).toEqual({ type: 'checkpoint', id: box })
        expect(init.headers).toMatchObject({ 'x-kiln-csrf': 'form-csrf' })
        if (posts === 1) {
          return Response.json(
            { id: request, box_id: box, state: 'running', result: null, error: null },
            { status: 202 },
          )
        }
        return Response.json({
          id: request,
          box_id: box,
          state: 'succeeded',
          result: { type: 'checkpoint', id: checkpoint, box_id: box },
          error: null,
        })
      }
      if (url.startsWith('/v1/operations/')) {
        return Response.json({
          id: request,
          box_id: box,
          state: 'succeeded',
          result: { type: 'checkpoint', id: checkpoint, box_id: box },
          error: null,
        })
      }
      reads += 1
      if (reads === 1) return Response.json({ ...previewDashboard, checkpoints: [] })
      return Response.json(seen)
    })
    const first = await createCheckpoint(box, fetcher, '__Host-kiln_csrf=form-csrf', request)
    const second = await createCheckpoint(box, fetcher, '__Host-kiln_csrf=form-csrf', request)
    expect(first.checkpoints.map((item) => item.id)).toEqual([checkpoint])
    expect(second.checkpoints.map((item) => item.id)).toEqual([checkpoint])
    expect(posts).toBe(2)
    expect(first.checkpoints.some((item) => item.box_id !== box)).toBe(false)
  })

  it('reports an unconfirmed checkpoint as still in progress and does not invent one', async () => {
    const fetcher = vi.fn(async (_input: RequestInfo | URL) =>
      Response.json({
        id: 'd'.repeat(32),
        box_id: 'c'.repeat(32),
        state: 'unknown',
        result: null,
        error: { message: 'Operation did not return a confirmed result.' },
      }),
    )
    await expect(
      createCheckpoint('c'.repeat(32), fetcher, '__Host-kiln_csrf=form-csrf', 'd'.repeat(32)),
    ).rejects.toThrow(/still in progress/)
    expect(fetcher.mock.calls.map(([url]) => String(url))).not.toContain('/api/v1/dashboard')
  })

  it('restores only the same box after acknowledgement and only once that box is observed', async () => {
    const request = 'e'.repeat(32)
    const box = 'c'.repeat(32)
    const checkpoint = 'b'.repeat(32)
    const other = 'f'.repeat(32)
    let reads = 0
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        expect(posted.id).toBe(request)
        expect(posted.action).toEqual({
          type: 'restore',
          id: box,
          checkpoint,
          acknowledge_external_state_replay: true,
        })
        return Response.json(
          {
            id: request,
            box_id: box,
            state: 'succeeded',
            result: { type: 'box', id: box },
            error: null,
          },
          { status: 202 },
        )
      }
      reads += 1
      const dashboard = structuredClone(previewDashboard)
      dashboard.boxes = [
        {
          ...previewDashboard.boxes[0],
          id: reads < 5 ? other : box,
          restored_from: checkpoint,
        },
      ]
      return Response.json(dashboard)
    })
    const observed = await restoreCheckpoint(
      box,
      checkpoint,
      fetcher,
      '__Host-kiln_csrf=form-csrf',
      request,
    )
    expect(observed.boxes.find((item) => item.id === box)?.restored_from).toBe(checkpoint)
    expect(reads).toBe(5)
  })

  it('submits pause, resume, stop, and start and shows state only once observed', async () => {
    const boxId = 'd'.repeat(32)
    const specs = [
      { action: 'pause', force: false, state: 'paused' },
      { action: 'resume', force: false, state: 'running' },
      { action: 'stop', force: false, state: 'stopped' },
      { action: 'stop', force: true, state: 'stopped' },
      { action: 'start', force: false, state: 'running' },
    ] as const
    for (const spec of specs) {
      const seen = structuredClone(previewDashboard)
      const box = { ...previewDashboard.boxes[0], id: boxId, state: spec.state }
      seen.boxes = [box]
      const withheld = spec.state === 'running' ? 'stopped' : 'running'
      let observed = false
      const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input)
        if (url === '/v1/operations' && init?.method === 'POST') {
          const posted = JSON.parse(String(init.body))
          expect(posted.id).toMatch(/^[a-f0-9]{32}$/)
          expect(posted.action).toEqual(
            spec.action === 'stop'
              ? { type: 'stop', id: boxId, force: spec.force }
              : { type: spec.action, id: boxId },
          )
          expect(init.headers).toMatchObject({ 'x-kiln-csrf': 'form-csrf' })
          expect(init.credentials).toBe('same-origin')
          return Response.json(
            {
              id: posted.id,
              box_id: boxId,
              state: 'running',
              result: { type: 'box', state: spec.state },
              error: null,
            },
            { status: 202 },
          )
        }
        if (url.startsWith('/v1/operations/')) {
          return Response.json({
            id: url.slice('/v1/operations/'.length),
            box_id: boxId,
            state: 'succeeded',
            result: { type: 'box', state: spec.state },
            error: null,
          })
        }
        if (!observed) {
          observed = true
          return Response.json({
            ...previewDashboard,
            boxes: [{ ...box, state: withheld }],
          })
        }
        return Response.json(seen)
      })
      const dashboard = await applyLifecycle(
        spec.action,
        boxId,
        spec.force,
        fetcher,
        '__Host-kiln_csrf=form-csrf',
      )
      expect(dashboard.boxes.find((item) => item.id === boxId)?.state).toBe(spec.state)
      expect(fetcher.mock.calls.filter(([url]) => String(url) === '/api/v1/dashboard').length).toBe(
        2,
      )
    }
  })

  it('reports a lifecycle still running at timeout without inventing a box state', async () => {
    const boxId = 'e'.repeat(32)
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json({
          id: posted.id,
          box_id: boxId,
          state: 'unknown',
          result: null,
          error: null,
        })
      }
      if (url.startsWith('/v1/operations/')) {
        return Response.json({
          id: url.slice('/v1/operations/'.length),
          box_id: boxId,
          state: 'unknown',
          result: null,
          error: null,
        })
      }
      return Response.json({
        ...previewDashboard,
        boxes: [{ ...previewDashboard.boxes[0], id: boxId, state: 'paused' }],
      })
    })
    await expect(
      applyLifecycle('pause', boxId, false, fetcher, '__Host-kiln_csrf=form-csrf'),
    ).rejects.toThrow(/still in progress/)
    expect(fetcher.mock.calls.some(([url]) => String(url) === '/api/v1/dashboard')).toBe(false)
  })

  it('submits delete and drops the box only once a later dashboard omits it', async () => {
    const boxId = 'f'.repeat(32)
    const kept = structuredClone(previewDashboard)
    kept.boxes = [{ ...previewDashboard.boxes[0], id: boxId }]
    const gone = structuredClone(previewDashboard)
    gone.boxes = []
    let observed = false
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        expect(posted.id).toMatch(/^[a-f0-9]{32}$/)
        expect(posted.action).toEqual({ type: 'delete', id: boxId })
        expect(init.headers).toMatchObject({ 'x-kiln-csrf': 'form-csrf' })
        expect(init.credentials).toBe('same-origin')
        return Response.json(
          { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
          { status: 202 },
        )
      }
      if (url.startsWith('/v1/operations/')) {
        return Response.json({
          id: url.slice('/v1/operations/'.length),
          box_id: boxId,
          state: 'succeeded',
          result: { type: 'deleted', id: boxId },
          error: null,
        })
      }
      if (!observed) {
        observed = true
        return Response.json(kept)
      }
      return Response.json(gone)
    })
    const dashboard = await deleteBox(boxId, fetcher, '__Host-kiln_csrf=form-csrf')
    expect(dashboard.boxes.some((box) => box.id === boxId)).toBe(false)
    expect(fetcher.mock.calls.filter(([url]) => String(url) === '/api/v1/dashboard').length).toBe(2)
  })

  it('reports delete still in progress without removing the box', async () => {
    const boxId = 'f'.repeat(32)
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json({
          id: posted.id,
          box_id: boxId,
          state: 'unknown',
          result: null,
          error: null,
        })
      }
      return Response.json({
        ...previewDashboard,
        boxes: [],
      })
    })
    await expect(deleteBox(boxId, fetcher, '__Host-kiln_csrf=form-csrf')).rejects.toThrow(
      /still in progress/,
    )
    expect(fetcher.mock.calls.some(([url]) => String(url) === '/api/v1/dashboard')).toBe(false)
  })

  it('posts one exec and returns the exit code, stdout and stderr', async () => {
    const boxId = 'a'.repeat(32)
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        expect(posted.id).toMatch(/^[a-f0-9]{32}$/)
        expect(posted.action).toEqual({
          type: 'exec',
          id: boxId,
          request: { argv: ['/bin/printf', 'hello'], timeout_ms: 1000, cwd: '/tmp' },
        })
        expect(init.headers).toMatchObject({ 'x-kiln-csrf': 'form-csrf' })
        expect(init.credentials).toBe('same-origin')
        return Response.json(
          {
            id: posted.id,
            box_id: boxId,
            state: 'succeeded',
            result: {
              type: 'exec',
              stdout: [...'hello'].map((char) => char.charCodeAt(0)),
              stderr: [...'err'].map((char) => char.charCodeAt(0)),
              exit_code: 7,
              truncated: true,
              timed_out: false,
            },
            error: null,
          },
          { status: 202 },
        )
      }
      return Response.json(previewDashboard)
    })
    const output = await execCommand(
      boxId,
      ['/bin/printf', 'hello'],
      1000,
      '/tmp',
      fetcher,
      '__Host-kiln_csrf=form-csrf',
    )
    expect(output).toEqual({
      exitCode: 7,
      stdout: 'hello',
      stderr: 'err',
      truncated: true,
      timedOut: false,
    })
    expect(fetcher.mock.calls.some(([url]) => String(url) === '/api/v1/dashboard')).toBe(false)
  })

  it('reports exec still in progress and a timeout without inventing output', async () => {
    const boxId = 'b'.repeat(32)
    const unknown = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      if (String(input) === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json({
          id: posted.id,
          box_id: boxId,
          state: 'unknown',
          result: null,
          error: null,
        })
      }
      return Response.json({ ...previewDashboard, boxes: [] })
    })
    await expect(
      execCommand(boxId, ['/bin/true'], 1000, undefined, unknown, '__Host-kiln_csrf=form-csrf'),
    ).rejects.toThrow(/still running|still in progress/)
    expect(unknown.mock.calls.some(([url]) => String(url) === '/api/v1/dashboard')).toBe(false)

    const timedOut = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      if (String(input) === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json({
          id: posted.id,
          box_id: boxId,
          state: 'succeeded',
          result: {
            type: 'exec',
            stdout: [],
            stderr: [],
            exit_code: null,
            truncated: true,
            timed_out: true,
          },
          error: null,
        })
      }
      return Response.json(previewDashboard)
    })
    const output = await execCommand(
      boxId,
      ['/bin/sleep', '30'],
      20,
      undefined,
      timedOut,
      '__Host-kiln_csrf=form-csrf',
    )
    expect(output.timedOut).toBe(true)
    expect(output.exitCode).toBeNull()
    expect(output.stdout).toBe('')
    expect(timedOut.mock.calls.some(([url]) => String(url) === '/api/v1/dashboard')).toBe(false)
  })
})

function failedOperationFetcher(boxId: string, message: string) {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    if (String(input) === '/v1/operations' && init?.method === 'POST') {
      const posted = JSON.parse(String(init.body))
      return Response.json({
        id: posted.id,
        box_id: boxId,
        state: 'failed',
        result: null,
        error: { message },
      })
    }
    return Response.json(previewDashboard)
  })
}

function runningOperationFetcher(boxId: string) {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    if (url === '/v1/operations' && init?.method === 'POST') {
      const posted = JSON.parse(String(init.body))
      return Response.json(
        { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
        { status: 202 },
      )
    }
    if (url.startsWith('/v1/operations/')) {
      const id = url.slice('/v1/operations/'.length)
      return Response.json({ id, box_id: boxId, state: 'running', result: null, error: null })
    }
    return Response.json(previewDashboard)
  })
}

const trackerCases = {
  create: {
    boxId: '1'.repeat(32),
    runSuccess: (fetcher: typeof fetch, _boxId: string) =>
      createBox('ubuntu', 'slow-boot', fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: { state: 'succeeded', result: null },
    dashboardDone: (boxId: string, _checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.boxes = [{ ...previewDashboard.boxes[0], id: boxId, name: 'slow-boot' }]
      return seen
    },
    dashboardPending: (_boxId?: string, _checkpointId?: string) => ({
      ...previewDashboard,
      boxes: [],
    }),
    assertSuccess: (result: unknown, boxId: string) => {
      expect((result as Dashboard).boxes.some((box) => box.id === boxId)).toBe(true)
    },
    failureMessage: 'Create rejected by the host.',
    inProgressAction: {
      type: 'create',
      template: 'ubuntu',
      name: 'never-finishes',
    } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  pause: {
    boxId: '2'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      applyLifecycle('pause', boxId, false, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: { state: 'succeeded', result: { type: 'box', state: 'paused' } },
    dashboardDone: (boxId: string, _checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.boxes = [{ ...previewDashboard.boxes[0], id: boxId, state: 'paused' }]
      return seen
    },
    dashboardPending: (boxId: string, _checkpointId?: string) => ({
      ...previewDashboard,
      boxes: [{ ...previewDashboard.boxes[0], id: boxId, state: 'running' }],
    }),
    assertSuccess: (result: unknown, boxId: string) => {
      expect((result as Dashboard).boxes.find((box) => box.id === boxId)?.state).toBe('paused')
    },
    failureMessage: 'The box is not running.',
    inProgressAction: { type: 'pause', id: '2'.repeat(32) } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  resume: {
    boxId: '3'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      applyLifecycle('resume', boxId, false, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: { state: 'succeeded', result: { type: 'box', state: 'running' } },
    dashboardDone: (boxId: string, _checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.boxes = [{ ...previewDashboard.boxes[0], id: boxId, state: 'running' }]
      return seen
    },
    dashboardPending: (boxId: string, _checkpointId?: string) => ({
      ...previewDashboard,
      boxes: [{ ...previewDashboard.boxes[0], id: boxId, state: 'paused' }],
    }),
    assertSuccess: (result: unknown, boxId: string) => {
      expect((result as Dashboard).boxes.find((box) => box.id === boxId)?.state).toBe('running')
    },
    failureMessage: 'The box is not paused.',
    inProgressAction: { type: 'resume', id: '3'.repeat(32) } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  stop: {
    boxId: '4'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      applyLifecycle('stop', boxId, false, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: { state: 'succeeded', result: { type: 'box', state: 'stopped' } },
    dashboardDone: (boxId: string, _checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.boxes = [{ ...previewDashboard.boxes[0], id: boxId, state: 'stopped' }]
      return seen
    },
    dashboardPending: (boxId: string, _checkpointId?: string) => ({
      ...previewDashboard,
      boxes: [{ ...previewDashboard.boxes[0], id: boxId, state: 'running' }],
    }),
    assertSuccess: (result: unknown, boxId: string) => {
      expect((result as Dashboard).boxes.find((box) => box.id === boxId)?.state).toBe('stopped')
    },
    failureMessage: 'Stop rejected by the host.',
    inProgressAction: { type: 'stop', id: '4'.repeat(32), force: false } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  start: {
    boxId: '5'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      applyLifecycle('start', boxId, false, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: { state: 'succeeded', result: { type: 'box', state: 'running' } },
    dashboardDone: (boxId: string, _checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.boxes = [{ ...previewDashboard.boxes[0], id: boxId, state: 'running' }]
      return seen
    },
    dashboardPending: (boxId: string, _checkpointId?: string) => ({
      ...previewDashboard,
      boxes: [{ ...previewDashboard.boxes[0], id: boxId, state: 'stopped' }],
    }),
    assertSuccess: (result: unknown, boxId: string) => {
      expect((result as Dashboard).boxes.find((box) => box.id === boxId)?.state).toBe('running')
    },
    failureMessage: 'Start rejected by the host.',
    inProgressAction: { type: 'start', id: '5'.repeat(32) } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  checkpoint: {
    boxId: '6'.repeat(32),
    checkpointId: '7'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      createCheckpoint(boxId, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: (boxId: string, checkpointId?: string) => ({
      state: 'succeeded',
      result: { type: 'checkpoint', id: checkpointId!, box_id: boxId },
    }),
    dashboardDone: (boxId: string, checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.checkpoints = [
        {
          id: checkpointId!,
          box_id: boxId,
          name: 'snap',
          created_at: previewDashboard.observed_at,
          size_bytes: 4096,
          compatible: true,
        },
      ]
      return seen
    },
    dashboardPending: (_boxId?: string, _checkpointId?: string) => ({
      ...previewDashboard,
      checkpoints: [],
    }),
    assertSuccess: (result: unknown, _boxId: string, checkpointId?: string) => {
      expect((result as Dashboard).checkpoints.some((item) => item.id === checkpointId)).toBe(true)
    },
    failureMessage: 'Checkpoint storage is full.',
    inProgressAction: { type: 'checkpoint', id: '6'.repeat(32) } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  restore: {
    boxId: '8'.repeat(32),
    checkpointId: '9'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string, checkpointId?: string) =>
      restoreCheckpoint(boxId, checkpointId!, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: (boxId: string) => ({ state: 'succeeded', result: { type: 'box', id: boxId } }),
    dashboardDone: (boxId: string, checkpointId?: string) => {
      const seen = structuredClone(previewDashboard)
      seen.boxes = [
        { ...previewDashboard.boxes[0], id: boxId, restored_from: checkpointId ?? null },
      ]
      return seen
    },
    dashboardPending: (boxId: string, _checkpointId?: string) => ({
      ...previewDashboard,
      boxes: [{ ...previewDashboard.boxes[0], id: boxId, restored_from: null }],
    }),
    assertSuccess: (result: unknown, boxId: string, checkpointId?: string) => {
      expect((result as Dashboard).boxes.find((box) => box.id === boxId)?.restored_from).toBe(
        checkpointId,
      )
    },
    failureMessage: 'Checkpoint is incompatible.',
    inProgressAction: {
      type: 'restore',
      id: '8'.repeat(32),
      checkpoint: '9'.repeat(32),
      acknowledge_external_state_replay: true,
    } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  delete: {
    boxId: 'a'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      deleteBox(boxId, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: (boxId: string) => ({
      state: 'succeeded',
      result: { type: 'deleted', id: boxId },
    }),
    dashboardDone: (_boxId: string, _checkpointId?: string) => {
      const gone = structuredClone(previewDashboard)
      gone.boxes = []
      return gone
    },
    dashboardPending: (boxId: string, _checkpointId?: string) => {
      const kept = structuredClone(previewDashboard)
      kept.boxes = [{ ...previewDashboard.boxes[0], id: boxId }]
      return kept
    },
    assertSuccess: (result: unknown, boxId: string) => {
      expect((result as Dashboard).boxes.some((box) => box.id === boxId)).toBe(false)
    },
    failureMessage: 'Delete rejected while a session is attached.',
    inProgressAction: { type: 'delete', id: 'a'.repeat(32) } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
  exec: {
    boxId: 'b'.repeat(32),
    runSuccess: (fetcher: typeof fetch, boxId: string) =>
      execCommand(boxId, ['/bin/true'], 1000, undefined, fetcher, '__Host-kiln_csrf=form-csrf'),
    slowSucceeded: {
      state: 'succeeded',
      result: {
        type: 'exec',
        stdout: [...'ok'].map((char) => char.charCodeAt(0)),
        stderr: [],
        exit_code: 0,
        truncated: false,
        timed_out: false,
      },
    },
    dashboardDone: (_boxId?: string, _checkpointId?: string) => previewDashboard,
    dashboardPending: (_boxId?: string, _checkpointId?: string) => previewDashboard,
    assertSuccess: (result: unknown, _boxId?: string, _checkpointId?: string) => {
      expect((result as { stdout: string }).stdout).toBe('ok')
    },
    failureMessage: 'Guest agent is unavailable.',
    inProgressAction: {
      type: 'exec',
      id: 'b'.repeat(32),
      request: { argv: ['/bin/true'], timeout_ms: 1000 },
    } satisfies OperationAction,
    inProgressWaitMs: 120,
  },
} satisfies Record<
  OperationActionType,
  {
    boxId: string
    checkpointId?: string
    runSuccess: (fetcher: typeof fetch, boxId: string, checkpointId?: string) => Promise<unknown>
    slowSucceeded:
      Record<string, unknown> | ((boxId: string, checkpointId?: string) => Record<string, unknown>)
    dashboardDone: (boxId: string, checkpointId?: string) => Dashboard
    dashboardPending: (boxId: string, checkpointId?: string) => Dashboard
    assertSuccess: (result: unknown, boxId: string, checkpointId?: string) => void
    failureMessage: string
    inProgressAction: OperationAction
    inProgressWaitMs: number
  }
>

describe('operation tracker', () => {
  it.each(Object.keys(trackerCases) as OperationActionType[])(
    'tracks %s through slow polls, host failure, and wait budget',
    async (kind) => {
      const spec = trackerCases[kind]
      const checkpointId = 'checkpointId' in spec ? spec.checkpointId : undefined
      const succeeded =
        typeof spec.slowSucceeded === 'function'
          ? spec.slowSucceeded(spec.boxId, checkpointId)
          : spec.slowSucceeded
      const fetcher = slowRunningFetcher(
        spec.boxId,
        8,
        succeeded,
        spec.dashboardDone(spec.boxId, checkpointId),
        spec.dashboardPending(spec.boxId, checkpointId),
      )
      const success = await spec.runSuccess(fetcher, spec.boxId, checkpointId)
      spec.assertSuccess(success, spec.boxId, checkpointId)

      const failed = failedOperationFetcher(spec.boxId, spec.failureMessage)
      await expect(spec.runSuccess(failed, spec.boxId, checkpointId)).rejects.toThrow(
        spec.failureMessage,
      )

      const running = runningOperationFetcher(spec.boxId)
      const inProgress =
        kind === 'exec'
          ? trackOperation(
              spec.inProgressAction as Extract<OperationAction, { type: 'exec' }>,
              running,
              '__Host-kiln_csrf=form-csrf',
              'c'.repeat(32),
              spec.inProgressWaitMs,
            )
          : trackOperation(
              spec.inProgressAction as Exclude<OperationAction, { type: 'exec' }>,
              running,
              '__Host-kiln_csrf=form-csrf',
              'c'.repeat(32),
              spec.inProgressWaitMs,
            )
      await expect(inProgress).rejects.toThrow(/still in progress|running/i)
      if (kind !== 'exec') {
        expect(running.mock.calls.some(([url]) => String(url) === '/api/v1/dashboard')).toBe(false)
      }
    },
  )

  it('uses trackOperation for CSRF and session rejection on submit', async () => {
    const fetcher = vi.fn(async () =>
      Response.json({ code: 'invalid_csrf', message: 'The request was refused.' }, { status: 403 }),
    )
    await expect(
      trackOperation({ type: 'delete', id: 'a'.repeat(32) }, fetcher, '__Host-kiln_csrf=form-csrf'),
    ).rejects.toThrow(/csrf/i)
    expect(fetcher).toHaveBeenCalledTimes(1)
  })
})
