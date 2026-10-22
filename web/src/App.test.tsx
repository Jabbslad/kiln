import { render, screen, within, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'

afterEach(() => {
  vi.useRealTimers()
})
import App from './App'
import { previewDashboard } from './fixtures'

afterEach(() => vi.unstubAllGlobals())

function open(path = '/boxes?preview=1') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  )
}

it('shows the server-reported version in the header', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(Response.json({ ...previewDashboard, version: '0.4.99' })),
  )
  open('/boxes')
  expect(await screen.findByText('v0.4.99')).toBeVisible()
})

it('opens embedded release notes when the version control is activated', async () => {
  open()
  const user = userEvent.setup()
  const version = await screen.findByRole('button', {
    name: new RegExp(`^Release notes for v${__KILN_BUILD_VERSION__.replace(/^v/, '')}$`),
  })
  await user.click(version)
  const dialog = screen.getByRole('dialog')
  expect(within(dialog).getByRole('heading', { level: 2 })).toHaveTextContent(/^v/)
  expect(within(dialog).getByRole('link', { name: /View on GitHub/i })).toHaveAttribute(
    'href',
    expect.stringMatching(/github\.com\/Jabbslad\/kiln\/releases\/tag\/v/),
  )
  expect(within(dialog).getAllByRole('listitem').length).toBeGreaterThan(0)
  await user.click(within(dialog).getByRole('button', { name: 'Close dialog' }))
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
})

it('shows a plain empty message when the running version has no embedded notes', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(Response.json({ ...previewDashboard, version: '9.9.9' })),
  )
  open('/boxes')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Release notes for v9.9.9' }))
  expect(screen.getByText('No release notes for v9.9.9')).toBeVisible()
})

it('keeps the public page independent of the host and makes preview an explicit choice', async () => {
  const request = vi.fn().mockRejectedValue(new Error('Host unavailable'))
  vi.stubGlobal('fetch', request)
  open('/')
  expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Your machine.')
  expect(screen.getByRole('link', { name: 'Open console' })).toHaveAttribute('href', '/boxes')
  expect(screen.queryByRole('link', { name: 'Sign in' })).not.toBeInTheDocument()
  expect(request).not.toHaveBeenCalled()
  await userEvent.setup().click(screen.getByRole('link', { name: 'Explore design preview' }))
  expect(await screen.findByRole('heading', { name: 'Boxes' })).toBeVisible()
  expect(screen.getByText('EXAMPLE DATA · NO VM OPERATIONS')).toBeVisible()
  expect(screen.queryByRole('link', { name: 'Sign in' })).not.toBeInTheDocument()
  expect(request).not.toHaveBeenCalled()
})

it('combines search and filters without mutating fixture state or fetching', async () => {
  const request = vi.fn()
  vi.stubGlobal('fetch', request)
  open()
  expect(await screen.findByRole('heading', { name: 'Boxes' })).toBeVisible()
  const user = userEvent.setup()
  await user.type(screen.getByRole('searchbox', { name: 'Search boxes' }), 'worker')
  expect(screen.getByRole('link', { name: 'worker-test' })).toBeVisible()
  expect(screen.queryByRole('link', { name: 'api-dev' })).not.toBeInTheDocument()
  await user.click(screen.getByRole('button', { name: 'Running 1' }))
  expect(screen.getByText('No matching boxes')).toBeVisible()
  await user.click(screen.getByRole('button', { name: 'Clear filters' }))
  expect(screen.getByRole('link', { name: 'api-dev' })).toBeVisible()
  expect(request).not.toHaveBeenCalled()
  expect(previewDashboard.boxes[1].state).toBe('paused')
})

it('shows a clear unreachable message, retry, and recovery after network failure', async () => {
  let calls = 0
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => {
      calls += 1
      if (calls === 1) throw new TypeError('Load failed')
      return Response.json(previewDashboard)
    }),
  )
  open('/boxes')
  expect(await screen.findByText("Couldn't reach the server")).toBeVisible()
  await userEvent.setup().click(screen.getByRole('button', { name: 'Retry connection' }))
  expect(await screen.findByRole('heading', { name: 'Boxes' })).toBeVisible()
  expect(calls).toBeGreaterThanOrEqual(2)
})

it('never substitutes examples when the service is down', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status: 503 })))
  open('/boxes')
  expect(await screen.findByRole('heading', { name: 'Connect your control plane' })).toBeVisible()
  expect(screen.queryByRole('link', { name: 'api-dev' })).not.toBeInTheDocument()
  expect(screen.getByRole('link', { name: 'Sign in' })).toHaveAttribute('href', '/login')
  expect(screen.getByText(/HTTP 503/)).toBeVisible()
})

it('renders an actually empty service response as empty, not disconnected', async () => {
  vi.stubGlobal(
    'fetch',
    vi
      .fn()
      .mockResolvedValue(
        Response.json({ ...previewDashboard, boxes: [], operations: [], checkpoints: [] }),
      ),
  )
  open('/boxes')
  expect(await screen.findByText('No boxes yet')).toBeVisible()
  expect(screen.queryByText('Connect your control plane')).not.toBeInTheDocument()
})

it('shows only same-box checkpoints and keeps restore gated after acknowledgement in preview', async () => {
  open('/boxes/box-7f3a2c1e?preview=1&tab=checkpoints')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Restore before-upgrade' }))
  const dialog = screen.getByRole('dialog')
  expect(within(dialog).getByText(/External actions are not undone/)).toBeVisible()
  expect(screen.queryByText('worker-baseline')).not.toBeInTheDocument()
  const restore = within(dialog).getByRole('button', { name: 'Restore checkpoint' })
  expect(restore).toBeDisabled()
  await user.click(
    within(dialog).getByRole('checkbox', { name: /I understand external state may replay/ }),
  )
  expect(restore).toBeDisabled()
  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
})

it('enables create only on a live authenticated console and shows the box once observed', async () => {
  const boxId = 'c'.repeat(32)
  const live = {
    ...previewDashboard,
    boxes: [],
    operations: [],
    checkpoints: [],
    templates: [{ ...previewDashboard.templates[0], id: 'tpl-ubuntu', name: 'ubuntu' }],
  }
  const observed = {
    ...live,
    boxes: [{ ...previewDashboard.boxes[0], id: boxId, name: 'api-staging' }],
  }
  let releaseObserved: (response: Response) => void = () => {}
  const observedResponse = new Promise<Response>((resolve) => {
    releaseObserved = resolve
  })
  let posts = 0
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        posts += 1
        const posted = JSON.parse(String(init.body))
        return Response.json(
          { id: posted.id, box_id: boxId, state: 'succeeded', result: null, error: null },
          { status: 202 },
        )
      }
      if (String(url).startsWith('/v1/operations/')) {
        return Response.json({
          id: 'ignored',
          box_id: boxId,
          state: 'succeeded',
          result: null,
          error: null,
        })
      }
      if (posts === 0) return Response.json(live)
      return observedResponse
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open('/boxes')
  const user = userEvent.setup()
  const [openCreate] = await screen.findAllByRole('button', { name: 'Create box' })
  await user.click(openCreate)
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Box name' }), 'api-staging')
  const submit = within(dialog).getByRole('button', { name: 'Create box' })
  expect(submit).toBeEnabled()
  await user.click(submit)
  await waitFor(() => expect(posts).toBe(1))
  expect(screen.queryByRole('link', { name: 'api-staging' })).not.toBeInTheDocument()
  releaseObserved(Response.json(observed))
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
  expect(await screen.findByRole('link', { name: 'api-staging' })).toBeVisible()
  expect(posts).toBe(1)
})

it('shows Creating… while create polls and closes with the new box once observed', async () => {
  const boxId = 'd'.repeat(32)
  const live = {
    ...previewDashboard,
    boxes: [],
    operations: [],
    checkpoints: [],
    templates: [{ ...previewDashboard.templates[0], id: 'tpl-ubuntu', name: 'ubuntu' }],
  }
  const observed = {
    ...live,
    boxes: [{ ...previewDashboard.boxes[0], id: boxId, name: 'jamie' }],
  }
  let pollCount = 0
  let createFinished = false
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json(
          { id: posted.id, box_id: boxId, state: 'running', result: null, error: null },
          { status: 202 },
        )
      }
      if (String(url).startsWith('/v1/operations/')) {
        pollCount += 1
        const id = String(url).slice('/v1/operations/'.length)
        if (pollCount < 8) {
          return Response.json({ id, box_id: boxId, state: 'running', result: null, error: null })
        }
        createFinished = true
        return Response.json({ id, box_id: boxId, state: 'succeeded', result: null, error: null })
      }
      return Response.json(createFinished ? observed : live)
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open('/boxes')
  const user = userEvent.setup()
  const [openCreate] = await screen.findAllByRole('button', { name: 'Create box' })
  await user.click(openCreate)
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Box name' }), 'jamie')
  await user.click(within(dialog).getByRole('button', { name: 'Create box' }))
  expect(await within(dialog).findByRole('button', { name: 'Creating…' })).toBeVisible()
  expect(within(dialog).getByRole('status')).toHaveTextContent(/Creating…/)
  expect(await screen.findByRole('link', { name: 'jamie' }, { timeout: 5000 })).toBeVisible()
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
  expect(pollCount).toBeGreaterThanOrEqual(8)
})

it('shows the host failure message when create returns failed', async () => {
  const live = {
    ...previewDashboard,
    boxes: [],
    operations: [],
    checkpoints: [],
    templates: [{ ...previewDashboard.templates[0], id: 'tpl-ubuntu', name: 'ubuntu' }],
  }
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json(
          {
            id: posted.id,
            box_id: 'f'.repeat(32),
            state: 'failed',
            result: null,
            error: { message: 'Template disk is unavailable.' },
          },
          { status: 202 },
        )
      }
      return Response.json(live)
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open('/boxes')
  const user = userEvent.setup()
  const [openCreate] = await screen.findAllByRole('button', { name: 'Create box' })
  await user.click(openCreate)
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Box name' }), 'jamie')
  await user.click(within(dialog).getByRole('button', { name: 'Create box' }))
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(
    'Template disk is unavailable.',
  )
  expect(within(dialog).queryByText(/still in progress/i)).not.toBeInTheDocument()
  expect(screen.getByRole('dialog')).toBeInTheDocument()
})

it('keeps create disabled when the live host is offline and keeps other mutations disabled', async () => {
  const offline = {
    ...previewDashboard,
    host: { ...previewDashboard.host, state: 'offline' as const },
  }
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(offline)))
  open('/boxes')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Create box' }))
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Box name' }), 'api-staging')
  expect(within(dialog).getByRole('button', { name: 'Create box' })).toBeDisabled()
  expect(within(dialog).getByText(/Read-only console/)).toBeVisible()
})

it('enables checkpoint confirmation on a live console', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(previewDashboard)))
  open('/boxes/box-7f3a2c1e')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Create checkpoint' }))
  const dialog = screen.getByRole('dialog')
  expect(within(dialog).getByRole('button', { name: 'Create checkpoint' })).toBeEnabled()
})

it.each([
  ['box-7f3a2c1e', 'Pause', 'pause', 'paused', 'running'],
  ['box-9c4d8e7b', 'Resume', 'resume', 'running', 'paused'],
  ['box-7f3a2c1e', 'Stop', 'stop', 'stopped', 'running'],
  ['box-a1b2c3d4', 'Start', 'start', 'running', 'stopped'],
] as const)(
  'submits %s once the console is live and shows %s only after the server observes it',
  async (boxId, label, type, nextState, previousState) => {
    const observed = structuredClone(previewDashboard)
    const box = observed.boxes.find((item) => item.id === boxId)
    if (!box) throw new Error(boxId)
    box.state = nextState
    let releaseObserved: (response: Response) => void = () => {}
    const observedResponse = new Promise<Response>((resolve) => {
      releaseObserved = resolve
    })
    let posts = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === '/v1/operations' && init?.method === 'POST') {
          posts += 1
          const posted = JSON.parse(String(init.body))
          expect(posted.action).toEqual(
            type === 'stop' ? { type, id: boxId, force: false } : { type, id: boxId },
          )
          return Response.json(
            {
              id: posted.id,
              box_id: boxId,
              state: 'succeeded',
              result: { type: 'box', state: nextState },
              error: null,
            },
            { status: 202 },
          )
        }
        if (String(url).startsWith('/v1/operations/')) {
          return Response.json({
            id: 'ignored',
            box_id: boxId,
            state: 'succeeded',
            result: { type: 'box', state: nextState },
            error: null,
          })
        }
        if (posts === 0) return Response.json(previewDashboard)
        return observedResponse
      }),
    )
    document.cookie = '__Host-kiln_csrf=browser-csrf'
    open(`/boxes/${boxId}`)
    const user = userEvent.setup()
    await user.click(await screen.findByRole('button', { name: label }))
    const dialog = screen.getByRole('dialog')
    const confirm = within(dialog).getByRole('button', { name: label })
    expect(confirm).toBeEnabled()
    await user.click(confirm)
    await waitFor(() => expect(posts).toBe(1))
    expect(screen.getByText(previousState, { selector: '.status' })).toBeVisible()
    releaseObserved(Response.json(observed))
    expect(await screen.findByText(nextState, { selector: '.status' })).toBeVisible()
    expect(posts).toBe(1)
  },
)

it('submits stop with force only when that option is chosen', async () => {
  const boxId = 'box-7f3a2c1e'
  let force: boolean | undefined
  let posts = 0
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        posts += 1
        const posted = JSON.parse(String(init.body))
        force = posted.action.force
        return Response.json(
          {
            id: posted.id,
            box_id: boxId,
            state: 'succeeded',
            result: { type: 'box', state: 'stopped' },
            error: null,
          },
          { status: 202 },
        )
      }
      if (String(url).startsWith('/v1/operations/')) {
        return Response.json({
          id: 'ignored',
          box_id: boxId,
          state: 'succeeded',
          result: { type: 'box', state: 'stopped' },
          error: null,
        })
      }
      if (posts === 0) return Response.json(previewDashboard)
      const observed = structuredClone(previewDashboard)
      const box = observed.boxes.find((item) => item.id === boxId)
      if (box) box.state = 'stopped'
      return Response.json(observed)
    }),
  )
  open(`/boxes/${boxId}`)
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Stop' }))
  const dialog = screen.getByRole('dialog')
  await user.click(within(dialog).getByRole('checkbox', { name: 'Force stop' }))
  await user.click(within(dialog).getByRole('button', { name: 'Stop' }))
  await waitFor(() => expect(force).toBe(true))
})

it('shows lifecycle still in progress and leaves the box state unchanged', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json({
          id: posted.id,
          box_id: 'box-7f3a2c1e',
          state: 'unknown',
          result: null,
          error: null,
        })
      }
      return Response.json(previewDashboard)
    }),
  )
  open('/boxes/box-7f3a2c1e')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Pause' }))
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Pause' }))
  expect(await screen.findByRole('alert')).toHaveTextContent(/still in progress/i)
  expect(screen.getByText('running', { selector: '.status' })).toBeVisible()
})

it('disables lifecycle confirmation when the console is not live or not authenticated', async () => {
  const user = userEvent.setup()
  const { unmount } = open('/boxes/box-7f3a2c1e?preview=1')
  await user.click(await screen.findByRole('button', { name: 'Pause' }))
  expect(within(screen.getByRole('dialog')).getByRole('button', { name: 'Pause' })).toBeDisabled()
  expect(within(screen.getByRole('dialog')).getByText(/Read-only preview/)).toBeVisible()
  unmount()

  const offline = {
    ...previewDashboard,
    host: { ...previewDashboard.host, state: 'offline' as const },
  }
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(offline)))
  open('/boxes/box-7f3a2c1e')
  await user.click(await screen.findByRole('button', { name: 'Stop' }))
  expect(within(screen.getByRole('dialog')).getByRole('button', { name: 'Stop' })).toBeDisabled()
  expect(within(screen.getByRole('dialog')).getByText(/Read-only console/)).toBeVisible()
})

it('does not submit delete until the typed name matches', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(previewDashboard)))
  open('/boxes/box-7f3a2c1e')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: /^Delete$/ }))
  const dialog = screen.getByRole('dialog')
  const confirm = within(dialog).getByRole('button', { name: 'Delete box' })
  const name = within(dialog).getByRole('textbox', { name: 'Type the box name' })
  expect(confirm).toBeDisabled()
  await user.type(name, 'API-DEV')
  expect(confirm).toBeDisabled()
  await user.clear(name)
  await user.type(name, 'api-de')
  expect(confirm).toBeDisabled()
  await user.type(name, 'v')
  expect(confirm).toBeEnabled()
})

it('submits delete and removes the box only after the server observes it gone', async () => {
  const boxId = 'box-7f3a2c1e'
  let releaseObserved: (response: Response) => void = () => {}
  const observedResponse = new Promise<Response>((resolve) => {
    releaseObserved = resolve
  })
  let posts = 0
  let observations = 0
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        posts += 1
        const posted = JSON.parse(String(init.body))
        expect(posted.action).toEqual({ type: 'delete', id: boxId })
        expect(init.credentials).toBe('same-origin')
        return Response.json(
          {
            id: posted.id,
            box_id: boxId,
            state: 'succeeded',
            result: { type: 'deleted', id: boxId },
            error: null,
          },
          { status: 202 },
        )
      }
      if (posts === 0) return Response.json(previewDashboard)
      observations += 1
      if (observations === 1) return Response.json(previewDashboard)
      return observedResponse
    }),
  )
  document.cookie = '__Host-kiln_csrf=browser-csrf'
  open(`/boxes/${boxId}`)
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: /^Delete$/ }))
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Type the box name' }), 'api-dev')
  await user.click(within(dialog).getByRole('button', { name: 'Delete box' }))
  await waitFor(() => expect(posts).toBe(1))
  expect(screen.getByRole('heading', { name: 'api-dev' })).toBeVisible()
  expect(screen.queryByText('Box not found')).not.toBeInTheDocument()
  const gone = structuredClone(previewDashboard)
  gone.boxes = gone.boxes.filter((box) => box.id !== boxId)
  releaseObserved(Response.json(gone))
  expect(await screen.findByText('Box not found')).toBeVisible()
  expect(posts).toBe(1)
})

it('shows delete still in progress and leaves the box in the inventory', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        const posted = JSON.parse(String(init.body))
        return Response.json({
          id: posted.id,
          box_id: 'box-7f3a2c1e',
          state: 'unknown',
          result: null,
          error: null,
        })
      }
      return Response.json(previewDashboard)
    }),
  )
  open('/boxes/box-7f3a2c1e')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: /^Delete$/ }))
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Type the box name' }), 'api-dev')
  await user.click(within(dialog).getByRole('button', { name: 'Delete box' }))
  expect(await screen.findByRole('alert')).toHaveTextContent(/still in progress/i)
  expect(screen.getByRole('heading', { name: 'api-dev' })).toBeVisible()
  expect(screen.queryByText('Box not found')).not.toBeInTheDocument()
})

it('disables delete when the console is not live or not authenticated', async () => {
  const user = userEvent.setup()
  const { unmount } = open('/boxes/box-7f3a2c1e?preview=1')
  await user.click(await screen.findByRole('button', { name: /^Delete$/ }))
  const previewDialog = screen.getByRole('dialog')
  await user.type(
    within(previewDialog).getByRole('textbox', { name: 'Type the box name' }),
    'api-dev',
  )
  expect(within(previewDialog).getByRole('button', { name: 'Delete box' })).toBeDisabled()
  expect(within(previewDialog).getByText(/Read-only preview/)).toBeVisible()
  unmount()

  const offline = {
    ...previewDashboard,
    host: { ...previewDashboard.host, state: 'offline' as const },
  }
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(offline)))
  open('/boxes/box-7f3a2c1e')
  await user.click(await screen.findByRole('button', { name: /^Delete$/ }))
  const dialog = screen.getByRole('dialog')
  await user.type(within(dialog).getByRole('textbox', { name: 'Type the box name' }), 'api-dev')
  expect(within(dialog).getByRole('button', { name: 'Delete box' })).toBeDisabled()
  expect(within(dialog).getByText(/Read-only console/)).toBeVisible()
})

it('disables exec in preview, when the host is offline or unknown, and when the box is stopped', async () => {
  const user = userEvent.setup()
  const { unmount } = open('/boxes/box-7f3a2c1e?preview=1&tab=exec')
  expect(await screen.findByRole('button', { name: /Execute command/ })).toBeDisabled()
  expect(screen.getByText(/Read-only preview/)).toBeVisible()
  unmount()

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
    const view = open('/boxes/box-7f3a2c1e?tab=exec')
    expect(await screen.findByRole('button', { name: /Execute command/ })).toBeDisabled()
    expect(screen.getByText(/Read-only console/)).toBeVisible()
    view.unmount()
  }

  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json(previewDashboard)))
  open('/boxes/box-a1b2c3d4?tab=exec')
  expect(await screen.findByRole('button', { name: /Execute command/ })).toBeDisabled()
  expect(screen.queryByText(/Read-only console/)).not.toBeInTheDocument()
})

it('shows the exit code, stdout and stderr after one exec', async () => {
  const boxId = 'box-7f3a2c1e'
  let posts = 0
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/v1/operations' && init?.method === 'POST') {
        posts += 1
        const posted = JSON.parse(String(init.body))
        expect(posted.action).toEqual({
          type: 'exec',
          id: boxId,
          request: { argv: ['/bin/pwd'], timeout_ms: 10000 },
        })
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
              exit_code: 0,
              truncated: false,
              timed_out: false,
            },
            error: null,
          },
          { status: 202 },
        )
      }
      return Response.json(previewDashboard)
    }),
  )
  open(`/boxes/${boxId}?tab=exec`)
  const user = userEvent.setup()
  const submit = await screen.findByRole('button', { name: /Execute command/ })
  expect(submit).toBeEnabled()
  await user.click(submit)
  expect(await screen.findByText('Exit code 0')).toBeVisible()
  expect(screen.getByText('hello')).toBeVisible()
  expect(screen.getByText('err')).toBeVisible()
  expect(posts).toBe(1)
})

it('validates a create form but cannot create a fake VM', async () => {
  open('/templates?preview=1')
  const user = userEvent.setup()
  await user.click(await screen.findByRole('button', { name: 'Create box' }))
  const dialog = screen.getByRole('dialog')
  const name = within(dialog).getByRole('textbox', { name: 'Box name' })
  await user.type(name, 'Invalid Name!')
  expect(within(dialog).getByText(/Use lowercase letters/)).toBeVisible()
  await user.clear(name)
  await user.type(name, 'api-staging')
  expect(within(dialog).queryByText(/Use lowercase letters/)).not.toBeInTheDocument()
  expect(within(dialog).getByRole('button', { name: 'Create box' })).toBeDisabled()
  expect(within(dialog).getByText(/Read-only preview/)).toBeVisible()
  expect(previewDashboard.boxes).toHaveLength(4)
})

it('marks the terminal capability available and keeps preview disconnected', async () => {
  open('/boxes/box-7f3a2c1e?preview=1')
  const terminal = await screen.findByTestId('capability-terminal')
  expect(terminal).toHaveAttribute('data-capability-state', 'available')
  expect(within(terminal).queryByText('Not available yet')).not.toBeInTheDocument()
  expect(within(terminal).getByText('Session not connected')).toBeVisible()
  expect(within(terminal).getByRole('button', { name: 'Connect terminal' })).toBeDisabled()
})

it('enables Connect terminal only for a running box on a live console', async () => {
  const dashboard = () => Promise.resolve(Response.json(previewDashboard))
  vi.stubGlobal('fetch', vi.fn().mockImplementation(dashboard))
  const running = open('/boxes/box-7f3a2c1e')
  expect(await screen.findByRole('button', { name: 'Connect terminal' })).toBeEnabled()
  running.unmount()

  const stopped = open('/boxes/box-a1b2c3d4')
  expect(await screen.findByRole('button', { name: 'Connect terminal' })).toBeDisabled()
  stopped.unmount()

  vi.stubGlobal(
    'fetch',
    vi.fn().mockImplementation(() =>
      Promise.resolve(
        Response.json({
          ...previewDashboard,
          host: { ...previewDashboard.host, state: 'offline' as const },
        }),
      ),
    ),
  )
  open('/boxes/box-7f3a2c1e')
  expect(await screen.findByRole('button', { name: 'Connect terminal' })).toBeDisabled()
})

it('posts sign-out to /logout with the CSRF value', async () => {
  const request = vi.fn(async (url: string, _init?: RequestInit) => {
    if (url === '/logout') return new Response(null, { status: 204 })
    return Response.json(previewDashboard)
  })
  vi.stubGlobal('fetch', request)
  document.cookie = '__Host-kiln_csrf=browser-csrf; Secure; Path=/'
  open('/boxes')
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Sign out' }))
  await waitFor(() => {
    const logout = request.mock.calls.find((call) => call[0] === '/logout')
    expect(logout?.[1]).toMatchObject({
      method: 'POST',
      credentials: 'same-origin',
      body: 'csrf=browser-csrf',
    })
  })
})
