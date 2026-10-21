#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
check=$root/scripts/check-feature-map.sh

fail() {
  printf 'test-feature-map-guard: %s\n' "$*" >&2
  exit 1
}

run_check() {
  body=$1
  shift
  if [ "$body" = - ]; then
    body=
  fi
  printf '%s\n' "$@" | (
    export PR_BODY="$body"
    sh "$check"
  )
}

run_check - README.md || fail 'docs-only path should pass'
run_check - web/src/data.ts && fail 'guarded path without map should fail'
run_check - crates/kiln-api/src/lib.rs && fail 'crate src without map should fail'
run_check - deploy/install.sh && fail 'deploy without map should fail'
run_check - .github/workflows/build.yml && fail 'workflow without map should fail'

run_check - web/src/data.ts .cursor/skills/verify-kiln/features/dashboard.md \
  || fail 'guarded path with feature file should pass'

run_check 'feature-map: n/a' web/src/App.tsx || fail 'escape hatch should pass'
run_check "$(printf 'other line\nfeature-map: n/a\nmore')" deploy/bootstrap/install.sh \
  || fail 'escape hatch as its own line should pass'
run_check 'prefix feature-map: n/a' web/src/App.tsx && fail 'inline escape must not count'

printf 'feature-map guard tests ok\n'
