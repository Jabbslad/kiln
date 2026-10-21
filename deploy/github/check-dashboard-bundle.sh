#!/bin/bash
# Fail unless the live dashboard HTML references the same index-*.js as ARCHIVE.
set -euo pipefail

fail() {
  printf 'dashboard-check: %s\n' "$*" >&2
  exit 1
}

archive=${1:?usage: check-dashboard-bundle.sh ARCHIVE}
[ -f "$archive" ] || fail "missing archive: ${archive}"

expected=$(
  tar -xOzf "$archive" index.html 2>/dev/null \
    | grep -oE '/assets/index-[A-Za-z0-9_-]+\.js' \
    | head -1
)
[ -n "$expected" ] || fail 'release index.html has no /assets/index-*.js script'
bundle=${expected#/assets/}
tar -tzf "$archive" | grep -Fxq "assets/${bundle}" \
  || fail "release archive is missing assets/${bundle}"

if [ -n "${KILN_DASHBOARD_LIVE_HTML:-}" ]; then
  live=$(
    grep -oE '/assets/index-[A-Za-z0-9_-]+\.js' "$KILN_DASHBOARD_LIVE_HTML" \
      | head -1
  )
else
  origin=${KILN_APP_ORIGIN:-https://app.dark-forge.dev}
  body=$(
    curl --fail --silent --show-error --location \
      --proto '=https' --proto-redir '=https' --max-time 30 \
      "${origin}/"
  )
  live=$(printf '%s' "$body" | grep -oE '/assets/index-[A-Za-z0-9_-]+\.js' | head -1)
fi

[ -n "$live" ] || fail 'live dashboard HTML has no /assets/index-*.js script'
[ "$live" = "$expected" ] || fail "live serves ${live}; release has ${expected}"
printf 'dashboard live %s\n' "$bundle"
