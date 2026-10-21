import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
import App from './App'
import { previewDashboard } from './fixtures'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

function open(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  )
}

it('renders only that box’s checkpoints from the dashboard payload', async () => {
  const payload = {
    ...previewDashboard,
    checkpoints: [
      {
        id: 'a'.repeat(32),
        box_id: 'box-7f3a2c1e',
        name: 'shift-snapshot',
        created_at: '2026-10-02T00:00:00Z',
        size_bytes: 4096,
        compatible: true,
      },
      {
        id: 'b'.repeat(32),
        box_id: 'box-9c4d8e7b',
        name: 'foreign-snapshot',
        created_at: '2026-10-02T00:00:00Z',
        size_bytes: 4096,
        compatible: true,
      },
    ],
  }
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(payload)))
  open('/boxes/box-7f3a2c1e?tab=checkpoints')
  expect(await screen.findByText('shift-snapshot')).toBeVisible()
  expect(screen.getByText('4 KiB')).toBeVisible()
  expect(screen.getByRole('button', { name: 'Restore shift-snapshot' })).toBeVisible()
  expect(screen.queryByRole('button', { name: 'Restore foreign-snapshot' })).not.toBeInTheDocument()
  expect(screen.queryByText('foreign-snapshot')).not.toBeInTheDocument()
})

it('keeps checkpoint and restore disabled when the console is not live or not authenticated', async () => {
  const request = vi.fn()
  vi.stubGlobal('fetch', request)
  const user = userEvent.setup()
  open('/boxes/box-7f3a2c1e?preview=1&tab=checkpoints')
  const [openCreate] = await screen.findAllByRole('button', { name: 'Create checkpoint' })
  await user.click(openCreate)
  let dialog = screen.getByRole('dialog')
  expect(within(dialog).getByRole('button', { name: 'Create checkpoint' })).toBeDisabled()
  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
  await user.click(screen.getByRole('button', { name: 'Restore before-upgrade' }))
  dialog = screen.getByRole('dialog')
  const restore = within(dialog).getByRole('button', { name: 'Restore checkpoint' })
  expect(restore).toBeDisabled()
  await user.click(
    within(dialog).getByRole('checkbox', { name: /I understand external state may replay/ }),
  )
  expect(restore).toBeDisabled()
  expect(request).not.toHaveBeenCalled()
  cleanup()

  for (const state of ['offline', 'unknown'] as const) {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        Response.json({
          ...previewDashboard,
          host: { ...previewDashboard.host, state },
        }),
      ),
    )
    open('/boxes/box-7f3a2c1e?tab=checkpoints')
    const user = userEvent.setup()
    const [create] = await screen.findAllByRole('button', { name: 'Create checkpoint' })
    await user.click(create)
    dialog = screen.getByRole('dialog')
    expect(within(dialog).getByRole('button', { name: 'Create checkpoint' })).toBeDisabled()
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    await user.click(screen.getByRole('button', { name: 'Restore before-upgrade' }))
    dialog = screen.getByRole('dialog')
    await user.click(
      within(dialog).getByRole('checkbox', { name: /I understand external state may replay/ }),
    )
    expect(within(dialog).getByRole('button', { name: 'Restore checkpoint' })).toBeDisabled()
    cleanup()
  }
})

it('submits one same-box checkpoint, shows still in progress, and retries that request id', async () => {
  const boxId = 'box-9c4d8e7b'
  const checkpointId = 'c'.repeat(32)
  const base = {
    ...previewDashboard,
    checkpoints: previewDashboard.checkpoints.filter((checkpoint) => checkpoint.box_id === boxId),
  }
  const observed = {
    ...base,
    checkpoints: [
      ...base.checkpoints,
      {
        id: checkpointId,
        box_id: boxId,
        name: 'shift-snapshot',
        created_at: '2026-10-02T00:00:00Z',
        size_bytes: 4096,
        compatible: true,
      },
    ],
  }
  const posts: Array<{ id: string; action: { type: string; id: string } }> = []
  let releaseObserved: (response: Response) => void = () => {}
  const observedResponse = new Promise<Response>((resolve) => {
    releaseObserved = resolve
  })
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        posts.push(posted)
        if (posts.length === 1) {
          return Response.json(
            { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
            { status: 202 },
          )
        }
        return Response.json({
          id: posted.id,
          box_id: boxId,
          state: 'succeeded',
          result: { type: 'checkpoint', id: checkpointId, box_id: boxId },
          error: null,
        })
      }
      if (String(url).startsWith('/v1/operations/')) {
        return Response.json({
          id: posts[0]?.id ?? 'pending',
          box_id: boxId,
          state: 'unknown',
          result: null,
          error: { message: 'Operation did not return a confirmed result.' },
        })
      }
      if (posts.length < 2) return Response.json(base)
      return observedResponse
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open(`/boxes/${boxId}?tab=checkpoints`)
  const user = userEvent.setup()
  expect(await screen.findByText('worker-baseline')).toBeVisible()
  expect(screen.queryByText('before-upgrade')).not.toBeInTheDocument()
  const [openCreate] = await screen.findAllByRole('button', { name: 'Create checkpoint' })
  await user.click(openCreate)
  const dialog = screen.getByRole('dialog')
  const submit = within(dialog).getByRole('button', { name: 'Create checkpoint' })
  expect(submit).toBeEnabled()
  await user.click(submit)
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(/still in progress/)
  expect(screen.queryByText('shift-snapshot')).not.toBeInTheDocument()
  await user.click(submit)
  await waitFor(() => expect(posts).toHaveLength(2))
  expect(posts[0].id).toMatch(/^[a-f0-9]{32}$/)
  expect(posts[1].id).toBe(posts[0].id)
  expect(posts[0].action).toEqual({ type: 'checkpoint', id: boxId })
  expect(posts[1].action).toEqual(posts[0].action)
  expect(screen.queryByText('shift-snapshot')).not.toBeInTheDocument()
  releaseObserved(Response.json(observed))
  expect(await screen.findByText('shift-snapshot')).toBeVisible()
  expect(posts).toHaveLength(2)
})

it('does not restore without the replay acknowledgement, then sends one same-box restore', async () => {
  const boxId = 'box-9c4d8e7b'
  const checkpointId = 'cp-worker'
  const base = structuredClone(previewDashboard)
  const box = base.boxes.find((item) => item.id === boxId)
  if (!box) throw new Error('missing fixture box')
  box.restored_from = null
  const observed = structuredClone(base)
  const restored = observed.boxes.find((item) => item.id === boxId)
  if (!restored) throw new Error('missing fixture box')
  restored.restored_from = checkpointId
  const posts: Array<{ id: string; action: Record<string, unknown> }> = []
  let releaseObserved: (response: Response) => void = () => {}
  const observedResponse = new Promise<Response>((resolve) => {
    releaseObserved = resolve
  })
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        posts.push(JSON.parse(String(init.body)))
        const posted = posts[0]
        return Response.json(
          {
            id: posted.id,
            box_id: boxId,
            state: 'succeeded',
            result: { type: 'box', id: boxId },
            error: null,
          },
          { status: 202 },
        )
      }
      if (posts.length === 0) return Response.json(base)
      return observedResponse
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open(`/boxes/${boxId}?tab=checkpoints`)
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Restore worker-baseline' }))
  const dialog = screen.getByRole('dialog')
  const restore = within(dialog).getByRole('button', { name: 'Restore checkpoint' })
  expect(restore).toBeDisabled()
  expect(posts).toHaveLength(0)
  await user.click(
    within(dialog).getByRole('checkbox', { name: /I understand external state may replay/ }),
  )
  expect(restore).toBeEnabled()
  await user.click(restore)
  await waitFor(() => expect(posts).toHaveLength(1))
  expect(posts[0].action).toEqual({
    type: 'restore',
    id: boxId,
    checkpoint: checkpointId,
    acknowledge_external_state_replay: true,
  })
  expect(screen.queryByText(/Restored from/)).not.toBeInTheDocument()
  releaseObserved(Response.json(observed))
  expect(await screen.findByText(checkpointId)).toBeVisible()
  expect(posts).toHaveLength(1)
})

it('shows a CSRF failure and a 401 without inventing a checkpoint', async () => {
  const boxId = 'box-9c4d8e7b'
  const responses = [
    Response.json({ code: 'invalid_csrf', message: 'The request was refused.' }, { status: 403 }),
    Response.json(
      { code: 'unauthorized', message: 'A valid credential is required.' },
      { status: 401 },
    ),
  ]
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') return responses.shift()!
      return Response.json(previewDashboard)
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open(`/boxes/${boxId}?tab=checkpoints`)
  const user = userEvent.setup()
  const [openCreate] = await screen.findAllByRole('button', { name: 'Create checkpoint' })
  await user.click(openCreate)
  const dialog = screen.getByRole('dialog')
  const submit = within(dialog).getByRole('button', { name: 'Create checkpoint' })
  await user.click(submit)
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(/csrf/i)
  expect(screen.queryByText('shift-snapshot')).not.toBeInTheDocument()
  await user.click(submit)
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(/401/)
  expect(screen.getByText('worker-baseline')).toBeVisible()
})
