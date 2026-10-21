import { test, expect } from '@playwright/test'
import { previewDashboard } from '../src/fixtures'

const artifactDir = process.env.KILN_SCREENSHOT_DIR

test('built dashboard shows unreachable state then recovers', async ({ page }) => {
  test.skip(!artifactDir, 'KILN_SCREENSHOT_DIR is required for proof capture')
  let release = false
  await page.route('**/api/v1/dashboard', async (route) => {
    if (!release) {
      await route.abort('connectionfailed')
      return
    }
    await route.fulfill({ json: previewDashboard })
  })
  await page.goto('/boxes')
  await expect(page.getByText("Couldn't reach the server")).toBeVisible()
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({ path: `${artifactDir}/network-unreachable-before.png`, fullPage: true })
  release = true
  await page.getByRole('button', { name: 'Retry connection' }).click()
  await expect(page.getByRole('heading', { name: 'Boxes' })).toBeVisible()
  await page.screenshot({ path: `${artifactDir}/network-unreachable-after.png`, fullPage: true })
})
