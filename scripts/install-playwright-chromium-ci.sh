#!/bin/sh
# Install Playwright chromium for CI with a per-attempt timeout and retries.
set -eu

attempts=${PLAYWRIGHT_INSTALL_ATTEMPTS:-3}
limit=${PLAYWRIGHT_INSTALL_TIMEOUT_SECS:-300}
n=1

while [ "$n" -le "$attempts" ]; do
  printf 'install-playwright-chromium-ci: attempt %s/%s (timeout %ss)\n' \
    "$n" "$attempts" "$limit" >&2
  if timeout "$limit" npx playwright install --with-deps chromium; then
    exit 0
  fi
  n=$((n + 1))
done

printf 'install-playwright-chromium-ci: failed after %s attempts\n' "$attempts" >&2
exit 1
