import { test, expect } from '@playwright/test'

const artifactDir = process.env.KILN_SCREENSHOT_DIR
const phase = process.env.KILN_PROOF_PHASE ?? 'after'

test('proof capture: landing page', async ({ page }) => {
  test.skip(!artifactDir, 'KILN_SCREENSHOT_DIR is required for proof capture')
  await page.goto('/')
  await expect(page).toHaveTitle('kiln — Linux boxes on your terms')
  await expect(page.getByRole('heading', { name: /Your machine/ })).toBeVisible()
  await expect(page.getByRole('link', { name: /design preview/i })).toHaveCount(0)
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: `${artifactDir}/${phase}-landing.png`,
    fullPage: true,
  })
})

test('proof capture: dashboard', async ({ page }) => {
  test.skip(!artifactDir, 'KILN_SCREENSHOT_DIR is required for proof capture')
  await page.goto('/boxes')
  await expect(page).toHaveTitle('Boxes — kiln')
  await expect(page.getByRole('heading', { name: 'Connect your control plane' })).toBeVisible()
  await expect(page.getByText('Design preview', { exact: true })).toHaveCount(0)
  await expect(page.getByRole('link', { name: /design preview/i })).toHaveCount(0)
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: `${artifactDir}/${phase}-dashboard-boxes.png`,
    fullPage: true,
  })
})
