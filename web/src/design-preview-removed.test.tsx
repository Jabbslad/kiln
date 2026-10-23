import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, it, vi } from 'vitest'
import App from './App'
import { previewDashboard } from './fixtures'

afterEach(() => vi.unstubAllGlobals())

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  )
}

function hrefs() {
  return [...document.querySelectorAll('a')].map((anchor) => anchor.getAttribute('href') ?? '')
}

it('removes design preview entry points and still renders the landing page and dashboard', async () => {
  const request = vi.fn().mockResolvedValue(Response.json(previewDashboard))
  vi.stubGlobal('fetch', request)

  const landing = renderAt('/')
  expect(screen.getByRole('heading', { level: 1, name: /Your machine/ })).toBeVisible()
  expect(screen.queryByRole('link', { name: /design preview/i })).not.toBeInTheDocument()
  expect(screen.queryByRole('link', { name: /exit preview/i })).not.toBeInTheDocument()
  expect(screen.queryByRole('switch', { name: /preview/i })).not.toBeInTheDocument()
  expect(screen.queryByRole('checkbox', { name: /design preview/i })).not.toBeInTheDocument()
  expect(hrefs().some((href) => href.includes('preview=1'))).toBe(false)
  expect(request).not.toHaveBeenCalled()
  landing.unmount()

  renderAt('/boxes?preview=1')
  expect(await screen.findByRole('heading', { name: 'Boxes' })).toBeVisible()
  expect(screen.queryByText('Design preview')).not.toBeInTheDocument()
  expect(screen.queryByText(/READ-ONLY DESIGN PREVIEW/)).not.toBeInTheDocument()
  expect(screen.queryByText(/EXAMPLE DATA/)).not.toBeInTheDocument()
  expect(screen.queryByRole('link', { name: /design preview/i })).not.toBeInTheDocument()
  expect(screen.queryByRole('link', { name: /exit preview/i })).not.toBeInTheDocument()
  expect(hrefs().some((href) => href.includes('preview=1'))).toBe(false)
  expect(request).toHaveBeenCalled()

  await userEvent.setup().click(screen.getByRole('link', { name: 'Settings' }))
  expect(await screen.findByRole('heading', { name: 'Settings' })).toBeVisible()
  expect(screen.queryByText('Design preview')).not.toBeInTheDocument()
  expect(screen.getByText('Read-only')).toBeVisible()
  expect(screen.queryByRole('switch', { name: /design preview/i })).not.toBeInTheDocument()
  expect(screen.queryByRole('checkbox', { name: /design preview/i })).not.toBeInTheDocument()
})
