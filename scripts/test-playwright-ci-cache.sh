#!/bin/sh
# Guard: the dashboard job caches Playwright browsers by lockfile version.
# A hit installs system packages only. A miss runs the full install.
# The install times out and retries once. This script does not use the network.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow=$root/.github/workflows/build.yml

fail() {
  printf 'test-playwright-ci-cache: %s\n' "$*" >&2
  exit 1
}

block=$(
  awk '/^  dashboard:/{flag=1} flag && /^  [a-z].*:/ && !/^  dashboard:/{exit} flag{print}' \
    "$workflow"
)

printf '%s\n' "$block" | grep -q 'path: ~/.cache/ms-playwright' \
  || fail 'dashboard must cache ~/.cache/ms-playwright'
printf '%s\n' "$block" | grep -q 'id: playwright-browsers' \
  || fail 'browser cache step needs id playwright-browsers'
printf '%s\n' "$block" | grep -q 'id: playwright-version' \
  || fail 'dashboard must resolve the Playwright version from the lockfile'
# shellcheck disable=SC2016 # GitHub Actions expression, matched literally
printf '%s\n' "$block" | grep -F 'key: ${{ runner.os }}-playwright-${{ steps.playwright-version.outputs.version }}' >/dev/null \
  || fail 'cache key must be the runner OS plus the resolved Playwright version'
if printf '%s\n' "$block" | grep -q 'hashFiles('; then
  fail 'playwright cache key must not hash the whole lockfile'
fi
printf '%s\n' "$block" | grep -q 'node_modules/playwright' \
  || fail 'version step must read node_modules/playwright from the dashboard lockfile'
printf '%s\n' "$block" | grep -q 'web/package-lock.json' \
  || fail 'version step must read web/package-lock.json'

install=$(
  printf '%s\n' "$block" | awk '
    /^      - name: Browser tests$/ && flag { exit }
    /^      - name: Install Playwright$/ { flag = 1 }
    flag { print }
  '
)
tests=$(
  printf '%s\n' "$block" | awk '
    /^      - name: Proof smoke$/ && flag { exit }
    /^      - name: Browser tests$/ { flag = 1 }
    flag { print }
  '
)

printf '%s\n' "$install" | grep -q 'steps.playwright-browsers.outputs.cache-hit' \
  || fail 'install must branch on the browser cache hit'
printf '%s\n' "$install" | grep -q 'npx playwright install-deps chromium' \
  || fail 'cache hit must run npx playwright install-deps chromium'
printf '%s\n' "$install" | grep -q 'npx playwright install --with-deps chromium' \
  || fail 'cache miss must run npx playwright install --with-deps chromium'
printf '%s\n' "$install" | grep -q 'sleep 2' \
  || fail 'retry must wait for apt to release its locks'
count=$(printf '%s\n' "$install" | grep -c 'timeout 300' || true)
[ "$count" -eq 2 ] || fail "install must time out and retry once (found $count timeout 300)"

printf '%s\n' "$tests" | grep -q 'npm run test:browser' \
  || fail 'browser tests must still run npm run test:browser'
if printf '%s\n' "$tests" | grep -q 'playwright install'; then
  fail 'browser tests must not install Playwright'
fi

grep -q 'sh scripts/test-playwright-ci-cache.sh' "$workflow" \
  || fail 'build.yml must run test-playwright-ci-cache.sh'

version=$(
  awk '
    index($0, "\"node_modules/playwright\":") { found = 1; next }
    found && index($0, "\"version\":") {
      line = $0
      sub(/.*"version": "/, "", line)
      sub(/".*/, "", line)
      print line
      exit
    }
  ' "$root/web/package-lock.json"
)
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "could not resolve playwright version from the dashboard lockfile (got '$version')"

printf 'playwright ci cache guard ok (%s)\n' "$version"
