import { test, expect } from '@playwright/test'

const artifactDir = process.env.KILN_SCREENSHOT_DIR

test('proof capture: version label opens embedded release notes', async ({ page }) => {
  test.skip(!artifactDir, 'KILN_SCREENSHOT_DIR is required for proof capture')
  await page.goto('/boxes?preview=1')
  await expect(page).toHaveTitle('Boxes — kiln')
  const version = page.getByRole('button', { name: /^Release notes for v/ })
  await expect(version).toBeVisible()
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: `${artifactDir}/before-dashboard-release-notes.png`,
    fullPage: true,
  })
  await version.click()
  const dialog = page.getByRole('dialog')
  await expect(dialog.getByRole('heading', { level: 2 })).toBeVisible()
  await expect(dialog.getByRole('link', { name: /View on GitHub/i })).toBeVisible()
  await page.screenshot({
    path: `${artifactDir}/after-dashboard-release-notes.png`,
    fullPage: true,
  })
})
