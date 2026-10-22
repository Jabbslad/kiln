import { render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
import App from './App'

afterEach(() => vi.unstubAllGlobals())

it('shows copyable kiln ssh commands with the full box id on Linux and macOS', async () => {
  vi.stubGlobal('fetch', vi.fn())
  const boxId = 'box-7f3a2c1e'
  render(
    <MemoryRouter initialEntries={[`/boxes/${boxId}?preview=1`]}>
      <App />
    </MemoryRouter>,
  )
  await screen.findByRole('heading', { name: /api-dev/i })
  const ssh = screen.getByTestId('capability-ssh')
  expect(ssh).toHaveAttribute('data-capability-state', 'available')
  expect(within(ssh).getByText(`kiln ssh ${boxId}`)).toBeVisible()
  expect(within(ssh).getByText(`kiln ssh-config ${boxId}`)).toBeVisible()
  expect(within(ssh).getByRole('button', { name: 'Copy kiln ssh command' })).toBeVisible()
  expect(within(ssh).getByRole('button', { name: 'Copy kiln ssh-config command' })).toBeVisible()
  expect(within(ssh).getByText('Use the Kiln CLI on Linux or macOS.')).toBeVisible()
  expect(within(ssh).queryByText('Not available yet')).not.toBeInTheDocument()
  expect(screen.queryByText(/SSH gateway/i)).not.toBeInTheDocument()
  expect(screen.queryByText(/Endpoint not configured/i)).not.toBeInTheDocument()
})
