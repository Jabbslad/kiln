import { cleanup, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { capabilities, type Capability } from './capabilities'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const PLACEHOLDER = /^[\s—\-–]*$|^—$|^-$|^N\/A$/i

function forbiddenPlaceholder(text: string) {
  const trimmed = text.trim()
  return trimmed === '' || PLACEHOLDER.test(trimmed)
}

function open(path: string) {
  vi.stubGlobal('fetch', vi.fn())
  render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  )
}

async function capabilityRoot(capability: Capability) {
  const nodes = await screen.findAllByTestId(`capability-${capability}`)
  return nodes
}

function assertNotYet(node: HTMLElement) {
  expect(node).toHaveAttribute('data-capability-state', 'not-yet')
  expect(within(node).getByText('Not available yet')).toBeVisible()
  const text = node.textContent ?? ''
  expect(text).not.toMatch(/Session not connected/)
  expect(text).not.toMatch(/Connect terminal/)
  expect(text).not.toMatch(/Awaiting host telemetry/)
  expect(text).not.toMatch(/Not reported/)
  within(node)
    .queryAllByRole('strong')
    .forEach((strong) => {
      expect(forbiddenPlaceholder(strong.textContent ?? '')).toBe(false)
    })
}

function assertAvailable(node: HTMLElement) {
  expect(node).toHaveAttribute('data-capability-state', 'available')
  expect(within(node).queryByText('Not available yet')).not.toBeInTheDocument()
  const text = node.textContent ?? ''
  expect(text).not.toMatch(/\bN\/A\b/i)
  within(node)
    .queryAllByRole('strong')
    .forEach((strong) => {
      expect(forbiddenPlaceholder(strong.textContent ?? '')).toBe(false)
    })
}

const panelPaths: Partial<Record<Capability, string[]>> = {
  metrics: ['/boxes?preview=1', '/boxes/box-7f3a2c1e?preview=1&tab=metrics'],
  terminal: ['/boxes/box-7f3a2c1e?preview=1'],
  ssh: ['/boxes/box-7f3a2c1e?preview=1'],
  httpPreview: ['/boxes/box-7f3a2c1e?preview=1&tab=previews'],
  publicUrl: ['/boxes/box-7f3a2c1e?preview=1'],
  launchTiming: ['/boxes/box-7f3a2c1e?preview=1', '/boxes/box-7f3a2c1e?preview=1&tab=metrics'],
}

describe('capability guard', () => {
  for (const capability of Object.keys(capabilities) as Capability[]) {
    const status = capabilities[capability]
    const paths = panelPaths[capability] ?? []
    if (paths.length === 0) continue

    it(`renders ${capability} as ${status} on its panels`, async () => {
      for (const path of paths) {
        open(path)
        if (path.includes('box-7f3a2c1e')) {
          await screen.findByRole('heading', { name: /api-dev/i })
        } else {
          await screen.findByRole('heading', { name: 'Boxes' })
        }
        const roots = await capabilityRoot(capability)
        expect(roots.length).toBeGreaterThan(0)
        for (const root of roots) {
          if (status === 'not-yet') assertNotYet(root)
          else assertAvailable(root)
        }
        cleanup()
      }
    })
  }
})
