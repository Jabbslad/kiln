import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
import App from './App'

afterEach(() => vi.unstubAllGlobals())

it('shows copyable kiln ssh commands with the full box id and no SSH gateway placeholder', async () => {
  vi.stubGlobal('fetch', vi.fn())
  const boxId = 'box-7f3a2c1e'
  render(
    <MemoryRouter initialEntries={[`/boxes/${boxId}?preview=1`]}>
      <App />
    </MemoryRouter>,
  )
  expect(await screen.findByText(`kiln ssh ${boxId}`)).toBeVisible()
  expect(screen.getByText(`kiln ssh-config ${boxId}`)).toBeVisible()
  expect(screen.queryByText(/SSH gateway/i)).not.toBeInTheDocument()
  expect(screen.queryByText(/Endpoint not configured/i)).not.toBeInTheDocument()
})
