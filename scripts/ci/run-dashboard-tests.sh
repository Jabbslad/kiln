#!/bin/sh
# dashboard test, build, and browser steps from .github/workflows/build.yml
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

(
  cd web
  npm ci
  npm test
  npx prettier --check .
  npm run build
  npx playwright install --with-deps chromium
  npm run test:browser
)
