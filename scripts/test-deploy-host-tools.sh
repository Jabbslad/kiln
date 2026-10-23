#!/bin/sh
# Production deploy post-checks must not assume host tools like jq or shell JSON scrapers.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-deploy-host-tools: %s\n' "$*" >&2
  exit 1
}

for script in deploy/github/*.sh; do
  base=$(basename "$script")
  if grep -E '(^|[^a-zA-Z0-9_-])jq([^a-zA-Z0-9_-]|$)' "$script" >/dev/null; then
    fail "$base invokes jq; use kiln-api self-check on the host instead"
  fi
  if grep -q 'check-dashboard-metrics\.sh' "$script"; then
    fail "$base still references removed check-dashboard-metrics.sh"
  fi
  if grep -q 'dashboard-metrics:' "$script"; then
    fail "$base still prints dashboard-metrics shell errors"
  fi
done

remote=$root/deploy/github/install-remote.sh
grep -q 'kiln-api self-check' "$remote" \
  || fail 'install-remote.sh must validate dashboard metrics with kiln-api self-check'

bash "$root/scripts/test-smoke-box.sh"

cargo xtask test-deploy-workflow >/dev/null \
  || fail 'deploy workflow tests failed'

printf 'deploy host tool guard ok\n'
