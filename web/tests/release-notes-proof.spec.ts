import { test, expect } from '@playwright/test'

const artifactDir = process.env.KILN_SCREENSHOT_DIR

test('proof capture: version label opens embedded release notes', async ({ page }) => {
  test.skip(!artifactDir, 'KILN_SCREENSHOT_DIR is required for proof capture')
  await page.goto('/boxes')
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
  const viewport = page.viewportSize()
  if (!viewport) throw new Error('viewport size is required')
  const backdropCoversViewport = await page.evaluate(({ width, height }) => {
    const dialog = document.querySelector('dialog.modal[open]')
    if (!dialog) return false
    const style = getComputedStyle(dialog, '::backdrop')
    if (style.display === 'none' || style.visibility === 'hidden') return false
    const backdropWidth = Number.parseFloat(style.width)
    const backdropHeight = Number.parseFloat(style.height)
    if (!Number.isFinite(backdropWidth) || !Number.isFinite(backdropHeight)) return false
    return (
      style.position === 'fixed' &&
      backdropWidth >= width - 1 &&
      backdropHeight >= height - 1 &&
      Number.parseFloat(style.top) <= 1 &&
      Number.parseFloat(style.left) <= 1
    )
  }, viewport)
  expect(backdropCoversViewport).toBe(true)
  await page.screenshot({
    path: `${artifactDir}/after-dashboard-release-notes.png`,
    fullPage: true,
  })
})
