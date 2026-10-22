import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './tests',
  testMatch: ['proof.spec.ts', 'network-unreachable-proof.spec.ts', 'release-notes-proof.spec.ts'],
  fullyParallel: false,
  workers: 1,
  retries: 0,
  use: {
    baseURL: process.env.KILN_PROOF_BASE_URL ?? 'http://127.0.0.1:4174',
    viewport: { width: 1440, height: 1000 },
    colorScheme: 'dark',
    reducedMotion: 'reduce',
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
      : {},
  },
})
