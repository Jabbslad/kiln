import { render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
import App from './App'

afterEach(() => vi.unstubAllGlobals())

it('labels SSH access as not available yet without kiln ssh command hints', async () => {
  vi.stubGlobal('fetch', vi.fn())
  render(
    <MemoryRouter initialEntries={['/boxes/box-7f3a2c1e?preview=1']}>
      <App />
    </MemoryRouter>,
  )
  await screen.findByRole('heading', { name: /api-dev/i })
  const ssh = screen.getByTestId('capability-ssh')
  expect(ssh).toHaveAttribute('data-capability-state', 'not-yet')
  expect(within(ssh).getByText('Coming soon')).toBeVisible()
  expect(screen.queryByText(/kiln ssh/i)).not.toBeInTheDocument()
})
