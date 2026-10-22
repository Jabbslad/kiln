#!/bin/sh
# Guard: dashboard npm ci uses setup-node cache and skips when web/ did not change.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow=$root/.github/workflows/build.yml

fail() {
  printf 'test-dashboard-npm-ci: %s\n' "$*" >&2
  exit 1
}

block=$(
  awk '/^  dashboard:/{flag=1} flag && /^  [a-z].*:/ && !/^  dashboard:/{exit} flag{print}' \
    "$workflow"
)
printf '%s\n' "$block" | grep -q 'npm ci' \
  || fail 'dashboard must install with npm ci'
printf '%s\n' "$block" | grep -q 'cache-dependency-path: web/package-lock.json' \
  || fail 'dashboard must cache npm from web/package-lock.json'
printf '%s\n' "$block" | grep -q 'needs.changes.outputs.web' \
  || fail 'dashboard npm must consult the web path filter'
printf '%s\n' "$block" | grep -q 'install-playwright-chromium-ci.sh' \
  || fail 'dashboard browser tests must install Playwright through the CI retry script'
printf '%s\n' "$block" | grep -q 'steps.playwright-version.outputs.version' \
  || fail 'dashboard Playwright cache must key on the Playwright version'

grep -q 'web=' "$root/xtask/src/ci_job_paths.rs" \
  || fail 'ci-job-paths must emit a web= output'

printf 'dashboard npm ci guard ok\n'
