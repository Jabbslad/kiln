import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

function kilnBuildVersion() {
  const stamped = process.env.VERSION?.replace(/^v/, '')
  if (stamped) return stamped
  const manifest = readFileSync(resolve(import.meta.dirname, '../Cargo.toml'), 'utf8')
  const match = manifest.match(/^version = "([^"]+)"/m)
  return match?.[1] ?? '0.0.0'
}

export default defineConfig({
  plugins: [react()],
  define: {
    __KILN_BUILD_VERSION__: JSON.stringify(kilnBuildVersion()),
  },
  build: { license: { fileName: 'THIRD_PARTY_LICENSES.md' } },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    restoreMocks: true,
  },
})
