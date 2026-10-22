import { test, expect } from '@playwright/test'

const artifactDir = process.env.KILN_SCREENSHOT_DIR
const phase = process.env.KILN_PROOF_PHASE ?? 'after'

test('proof capture: built preview inventory', async ({ page }) => {
  test.skip(!artifactDir, 'KILN_SCREENSHOT_DIR is required for proof capture')
  await page.goto('/boxes?preview=1')
  await expect(page).toHaveTitle('Boxes — kiln')
  await expect(page.getByRole('heading', { name: 'Boxes' })).toBeVisible()
  await expect(page.getByText('Design preview', { exact: true })).toBeVisible()
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: `${artifactDir}/${phase}-dashboard-boxes.png`,
    fullPage: true,
  })

  await page.goto('/boxes/box-7f3a2c1e?preview=1')
  await expect(page.getByRole('heading', { name: /api-dev/i })).toBeVisible()
  await expect(page.getByTestId('capability-ssh')).toBeVisible()
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: `${artifactDir}/${phase}-dashboard-access-ssh.png`,
    fullPage: true,
  })
})
