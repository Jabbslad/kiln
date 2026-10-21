#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
check=$root/scripts/check-pr-proof.sh

fail() {
  printf 'test-check-pr-proof: %s\n' "$*" >&2
  exit 1
}

run_check() {
  body=$1
  shift
  printf '%s\n' "$@" | (
    export PR_BODY="$body"
    sh "$check"
  )
}

run_check - README.md || fail 'docs-only path should pass'
run_check - web/src/App.tsx && fail 'guarded path without proof should fail'

run_check $'## Proof\n\n![dashboard](artifacts/proof/after-dashboard-boxes.png)' web/src/App.tsx \
  || fail 'markdown image proof should pass'

run_check 'proof: n/a' web/src/App.tsx || fail 'escape hatch should pass'
run_check "$(printf 'other\nproof: n/a\n')" crates/kiln-client/src/main.rs \
  || fail 'escape hatch on its own line should pass'
run_check 'prefix proof: n/a' web/src/App.tsx && fail 'inline escape must not count'

run_check $'Proof\n![](demo.webm)' crates/kiln-runtime/src/bin/kiln-runtime.rs \
  || fail 'video extension link should pass'

printf 'pr-proof guard tests ok\n'
