import { render, screen } from '@testing-library/react'
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
  expect(await screen.findByText(`kiln ssh ${boxId}`)).toBeVisible()
  expect(screen.getByText(`kiln ssh-config ${boxId}`)).toBeVisible()
  expect(screen.getByRole('button', { name: 'Copy kiln ssh command' })).toBeVisible()
  expect(screen.getByRole('button', { name: 'Copy kiln ssh-config command' })).toBeVisible()
  expect(screen.getByText('Use the Kiln CLI on Linux or macOS.')).toBeVisible()
  const sshSection = screen.getByText('SSH').closest('.access-section')
  expect(sshSection).not.toBeNull()
  expect(sshSection).not.toHaveTextContent('Not available yet')
  expect(screen.queryByText(/SSH gateway/i)).not.toBeInTheDocument()
  expect(screen.queryByText(/Endpoint not configured/i)).not.toBeInTheDocument()
})
