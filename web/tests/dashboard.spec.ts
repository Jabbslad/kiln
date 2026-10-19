import { test, expect, type Page } from '@playwright/test'
import AxeBuilder from '@axe-core/playwright'
import { previewDashboard } from '../src/fixtures'

const artifactDir = process.env.KILN_SCREENSHOT_DIR
async function capture(page: Page, name: string) {
  if (artifactDir) {
    await page.evaluate(() => document.fonts.ready)
    await page.screenshot({ path: `${artifactDir}/${name}.png`, fullPage: true })
  }
}

test('public page copies only the client installer and separates live console from preview', async ({
  page,
  context,
}) => {
  const apiRequests: string[] = []
  page.on('request', (request) => {
    if (request.url().includes('/api/')) apiRequests.push(request.url())
  })
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await page.goto('/')
  await expect(page).toHaveTitle('kiln — Linux boxes on your terms')
  await page.getByRole('link', { name: 'Install Kiln', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'A terminal is all you need.' })).toBeInViewport()
  await page.getByRole('button', { name: 'Copy client install command' }).click()
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    'curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh',
  )
  await expect(page.getByRole('status')).toHaveText('Copied')
  await page.getByRole('link', { name: 'Explore design preview' }).click()
  await expect(page).toHaveURL(/\/boxes\?preview=1$/)
  await expect(page.getByText('EXAMPLE DATA · NO VM OPERATIONS')).toBeVisible()
  expect(apiRequests).toEqual([])
  await page.getByRole('link', { name: 'kiln home' }).click()
  await page.getByRole('link', { name: 'Open console' }).click()
  await expect(page).toHaveURL(/\/boxes$/)
  await expect(page.getByRole('heading', { name: 'Connect your control plane' })).toBeVisible()
  await capture(page, 'default-disconnected')
})

test('public page is readable across breakpoints and explains clipboard denial', async ({
  page,
}) => {
  await page.goto('/')
  for (const width of [1440, 1280, 1024, 768, 390, 320]) {
    await page.setViewportSize({ width, height: width < 768 ? 844 : 1000 })
    await page.evaluate(() => document.fonts.ready)
    await page.locator('.landing-hero img').evaluate((image: HTMLImageElement) => image.decode())
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await expect(page.getByRole('link', { name: 'Open console' })).toBeInViewport()
    if (width === 1440 || width === 390) {
      await capture(page, `landing-${width}`)
      expect(
        (await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze())
          .violations,
      ).toEqual([])
    }
  }
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: () => Promise.reject(new Error('Denied')) },
      configurable: true,
    })
  })
  await page.getByRole('button', { name: 'Copy client install command' }).click()
  await expect(page.getByRole('status')).toHaveText('Copy unavailable. Select the text to copy.')
  await expect(page.locator('.install-command code')).toContainText('curl -fsSL')
})

test('inventory, search, sorting, navigation and actual rendered design', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const requests: string[] = []
  page.on('request', (request) => {
    if (request.url().includes('/api/')) requests.push(request.url())
  })
  await page.goto('/boxes?preview=1')
  await expect(page).toHaveTitle('Boxes — kiln')
  await expect(page.getByRole('link', { name: 'kiln home' })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Boxes' })).toBeVisible()
  await capture(page, 'dashboard-desktop')
  await page.getByRole('searchbox').fill('worker')
  await expect(page.getByRole('link', { name: 'worker-test', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Running 1' }).click()
  await expect(page.getByText('No matching boxes')).toBeVisible()
  await page.getByRole('button', { name: 'Clear filters' }).click()
  await page.getByRole('button', { name: 'Name' }).click()
  await expect(page.locator('tbody tr').first()).toContainText('worker-test')
  await page.getByRole('link', { name: 'api-dev', exact: true }).click()
  await expect(page.getByText('Session not connected')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Connect terminal' })).toBeDisabled()
  await capture(page, 'box-workspace-desktop')
  await page.getByRole('button', { name: 'Exec', exact: true }).click()
  await page.getByLabel('Arguments (JSON array)').fill('[42]')
  await expect(page.getByLabel('Arguments (JSON array)')).toHaveAttribute('aria-invalid', 'true')
  await page.getByLabel('Arguments (JSON array)').fill('["/bin/echo", "hello"]')
  await expect(page.getByLabel('Arguments (JSON array)')).toHaveAttribute('aria-invalid', 'false')
  expect(requests).toEqual([])
  expect(errors).toEqual([])
})

test('template drawer validates, traps focus, closes with Escape and never submits', async ({
  page,
}) => {
  await page.goto('/templates?preview=1')
  await page.getByRole('button', { name: 'Create box', exact: true }).click()
  const dialog = page.getByRole('dialog')
  await expect(dialog).toBeVisible()
  await page.getByLabel('Box name').fill('My Box!')
  await expect(page.getByText(/Use lowercase letters/)).toBeVisible()
  await page.getByLabel('Box name').fill('api-dev')
  await expect(page.getByText('A box with this name already exists.')).toBeVisible()
  await page.getByLabel('Box name').fill('api-staging')
  await expect(dialog.getByRole('button', { name: 'Create box' })).toBeDisabled()
  await expect(page.getByText(/Read-only preview/)).toBeVisible()
  await capture(page, 'create-drawer-desktop')
  await dialog.getByRole('button', { name: 'Cancel' }).focus()
  await page.keyboard.press('Tab')
  expect(await page.evaluate(() => !!document.activeElement?.closest('dialog'))).toBe(true)
  await page.keyboard.press('Escape')
  await expect(dialog).not.toBeVisible()
  await expect(page.getByRole('button', { name: 'Create box', exact: true })).toBeFocused()
})

test('restore acknowledgement stays gated and checkpoint scope survives reload', async ({
  page,
}) => {
  await page.goto('/boxes/box-7f3a2c1e?preview=1&tab=checkpoints')
  await page.reload()
  await expect(page.getByText('worker-baseline')).toHaveCount(0)
  await page.getByRole('button', { name: 'Restore before-upgrade' }).click()
  await expect(page.getByText(/External actions are not undone/)).toBeVisible()
  await page.getByRole('checkbox').check()
  await expect(page.getByRole('button', { name: 'Restore checkpoint', exact: true })).toBeDisabled()
  await capture(page, 'restore-dialog-desktop')
  await page.keyboard.press('Escape')
  await expect(page.getByRole('button', { name: 'Restore before-upgrade' })).toBeFocused()
})

test('live loading, empty, zero metrics and service failure are distinct', async ({ page }) => {
  let release: () => void = () => {}
  const pending = new Promise<void>((resolve) => {
    release = resolve
  })
  await page.route('**/api/v1/dashboard', async (route) => {
    await pending
    await route.fulfill({
      json: {
        ...previewDashboard,
        boxes: [],
        checkpoints: [],
        operations: [],
        host: {
          ...previewDashboard.host,
          metrics: { cpu_percent: 0, memory_bytes: 0, disk_bytes: null },
        },
      },
    })
  })
  await page.goto('/boxes')
  await expect(page.getByRole('heading', { name: 'Fetching box state…' })).toBeVisible()
  await capture(page, 'loading-desktop')
  release()
  await expect(page.getByText('No boxes yet')).toBeVisible()
  await expect(page.getByText('0%', { exact: true })).toBeVisible()
  await expect(page.getByText('0 B', { exact: true })).toBeVisible()
  await capture(page, 'empty-desktop')
  await page.unroute('**/api/v1/dashboard')
  await page.route('**/api/v1/dashboard', (route) =>
    route.fulfill({ status: 503, body: 'Unavailable' }),
  )
  await page.getByRole('button', { name: 'Refresh dashboard' }).click()
  await expect(page.getByRole('heading', { name: 'Connect your control plane' })).toBeVisible()
  await expect(page.getByText('No boxes yet')).toHaveCount(0)
  await capture(page, 'disconnected-desktop')
})

test('paused state and failed operation details do not fabricate lifecycle success', async ({
  page,
}) => {
  await page.goto('/boxes/box-9c4d8e7b?preview=1')
  await expect(page.getByText('Pausing does not free memory.')).toBeVisible()
  await page.getByRole('button', { name: 'Resume', exact: true }).click()
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Resume' })).toBeDisabled()
  await page.keyboard.press('Escape')
  await capture(page, 'paused-desktop')
  await page.goto('/operations?preview=1&operation=op-21b8')
  await expect(
    page.getByText('Template is incompatible with this host. No VM was started.'),
  ).toBeVisible()
  await capture(page, 'operations-desktop')
})

test('mobile layout and navigation remain usable without document overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/boxes?preview=1')
  await expect(page.getByRole('heading', { name: 'Boxes' })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(
    await page
      .locator('.table-scroll')
      .evaluate((element) => element.scrollWidth <= element.clientWidth),
  ).toBe(true)
  await expect(page.getByRole('link', { name: 'Open api-dev', exact: true })).toBeInViewport()
  expect(
    await page
      .getByRole('searchbox')
      .evaluate((element) => parseFloat(getComputedStyle(element).fontSize)),
  ).toBeGreaterThanOrEqual(16)
  const navigation = page.getByRole('navigation', { name: 'Main navigation' })
  await expect(navigation).toBeInViewport()
  for (const link of await navigation.getByRole('link').all()) {
    const bounds = await link.boundingBox()
    expect(bounds!.height).toBeGreaterThanOrEqual(44)
  }
  await page.getByRole('button', { name: 'Sort boxes by name' }).click()
  await expect(page.locator('tbody tr').first()).toContainText('worker-test')
  await page.getByRole('button', { name: 'Sort boxes by name' }).click()
  await capture(page, 'dashboard-mobile')
  await page.getByRole('link', { name: 'Templates', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Templates' })).toBeVisible()
  await expect(navigation.getByRole('link', { name: 'Templates', exact: true })).toHaveAttribute(
    'aria-current',
    'page',
  )
  await page.getByRole('button', { name: 'Create box', exact: true }).click()
  await expect(page.getByRole('dialog')).toBeVisible()
  await page.getByLabel('Box name').fill('mobile-dev')
  for (const field of [
    page.getByLabel('Box name'),
    page.getByLabel('Prepared template', { exact: true }),
  ]) {
    expect(
      await field.evaluate((element) => parseFloat(getComputedStyle(element).fontSize)),
    ).toBeGreaterThanOrEqual(16)
  }
  await capture(page, 'create-drawer-mobile')
  expect(
    await page.evaluate(() => document.querySelector('dialog')!.scrollWidth <= innerWidth),
  ).toBe(true)
  expect(
    await page.evaluate(() => {
      const body = document.querySelector('dialog .modal-body')!.getBoundingClientRect()
      const footer = document.querySelector('dialog .modal-footer')!.getBoundingClientRect()
      return body.bottom <= footer.top + 1
    }),
  ).toBe(true)
  await page.getByText('No secrets attached').scrollIntoViewIfNeeded()
  await expect(page.getByText('No secrets attached')).toBeInViewport()
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Cancel' })).toBeInViewport()
  await capture(page, 'create-drawer-mobile-scrolled')
})

for (const width of [320, 430]) {
  test(`phone layout at ${width}px supports row taps, filters and persistent navigation`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 844 })
    await page.goto('/boxes?preview=1')
    await page.getByRole('button', { name: 'Failed 1' }).click()
    await expect(page.getByRole('link', { name: 'sandbox', exact: true })).toBeVisible()
    await page.getByRole('link', { name: 'Open sandbox' }).click()
    await expect(page.getByRole('button', { name: 'Activity', exact: true })).toHaveAttribute(
      'aria-current',
      'page',
    )
    await page.goto('/boxes?preview=1')
    await page
      .locator('tbody tr')
      .first()
      .click({ position: { x: 30, y: 65 } })
    await expect(page.getByRole('heading', { name: 'api-dev', exact: true })).toBeVisible()
    for (const path of [
      '/boxes?preview=1',
      '/boxes/box-7f3a2c1e?preview=1',
      '/templates?preview=1',
      '/operations?preview=1',
      '/settings?preview=1',
    ]) {
      await page.goto(path)
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      )
      await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight))
      await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeInViewport()
    }
  })
}

test('phone inventory retains accessible table semantics after reflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/boxes?preview=1')
  await expect(page.getByRole('table')).toHaveCount(1)
  await expect(page.getByRole('row')).toHaveCount(5)
  await expect(page.getByRole('columnheader', { name: 'Name', exact: true })).toHaveAttribute(
    'aria-sort',
    'ascending',
  )
  expect(
    (await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze())
      .violations,
  ).toEqual([])
})

test('key screens have no automated accessibility violations', async ({ page }) => {
  for (const path of [
    '/boxes?preview=1',
    '/templates?preview=1',
    '/boxes/box-7f3a2c1e?preview=1',
    '/operations?preview=1',
    '/settings?preview=1',
    '/docs?preview=1',
  ]) {
    await page.goto(path)
    await expect(page.locator('main h1')).toBeVisible()
    expect(
      (await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze())
        .violations,
    ).toEqual([])
  }
  await page.goto('/templates?preview=1')
  await page.getByRole('button', { name: 'Create box', exact: true }).click()
  expect(
    (await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze())
      .violations,
  ).toEqual([])
})
