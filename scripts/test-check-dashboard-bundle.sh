#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check-dashboard-bundle: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/pkg/assets"
printf '<script type="module" crossorigin src="/assets/index-AbCdEf.js"></script>\n' \
  >"$work/pkg/index.html"
printf 'js\n' >"$work/pkg/assets/index-AbCdEf.js"
tar -C "$work/pkg" -czf "$work/dashboard.tar.gz" index.html assets/index-AbCdEf.js

if bash deploy/github/check-dashboard-bundle.sh "$work/dashboard.tar.gz" >/dev/null 2>&1; then
  fail 'live check must fail without KILN_DASHBOARD_LIVE_HTML'
fi

KILN_DASHBOARD_LIVE_HTML="$work/pkg/index.html" \
  bash deploy/github/check-dashboard-bundle.sh "$work/dashboard.tar.gz" >"$work/out" \
  || fail 'matching live html should pass'
grep -q 'dashboard live index-AbCdEf.js' "$work/out" || fail 'missing success line'

printf '<script type="module" crossorigin src="/assets/index-Stale.js"></script>\n' \
  >"$work/stale.html"
if KILN_DASHBOARD_LIVE_HTML="$work/stale.html" \
  bash deploy/github/check-dashboard-bundle.sh "$work/dashboard.tar.gz" >/dev/null 2>&1; then
  fail 'stale live html must fail'
fi

printf 'check-dashboard-bundle tests ok\n'
