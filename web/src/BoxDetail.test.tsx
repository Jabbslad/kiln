import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
import App from './App'

afterEach(() => vi.unstubAllGlobals())

it('labels SSH access as not available yet without kiln ssh command hints', async () => {
  vi.stubGlobal('fetch', vi.fn())
  const boxId = 'box-7f3a2c1e'
  render(
    <MemoryRouter initialEntries={[`/boxes/${boxId}?preview=1`]}>
      <App />
    </MemoryRouter>,
  )
  await screen.findByRole('heading', { name: /api-dev/i })
  const sshSection = screen.getByText('SSH').closest('.access-section')
  expect(sshSection).not.toBeNull()
  expect(sshSection).toHaveTextContent('Not available yet')
  expect(screen.queryByText(/kiln ssh/i)).not.toBeInTheDocument()
  expect(screen.queryByText(/SSH gateway/i)).not.toBeInTheDocument()
  expect(screen.queryByText(/Endpoint not configured/i)).not.toBeInTheDocument()
})
