import { render, screen, within, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
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

it('never substitutes examples when the service is down', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status: 503 })))
  open('/boxes')
  expect(await screen.findByRole('heading', { name: 'Connect your control plane' })).toBeVisible()
  expect(screen.queryByRole('link', { name: 'api-dev' })).not.toBeInTheDocument()
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
    within(dialog).getByRole('checkbox', { name: 'I understand the rollback risks.' }),
  )
  expect(restore).toBeDisabled()
  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
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

it('does not present an interactive terminal session as connected', async () => {
  open('/boxes/box-7f3a2c1e?preview=1')
  expect(await screen.findByText('Session not connected')).toBeVisible()
  expect(screen.queryByText('Connected', { exact: true })).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: 'Connect terminal' })).toBeDisabled()
})
