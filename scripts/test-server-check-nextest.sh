#!/bin/sh
# Guard: server-check uses cargo-nextest with parallel partitions on one runner.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow=$root/.github/workflows/build.yml
check=$root/scripts/check.sh
runner=$root/scripts/run-workspace-nextest.sh

fail() {
  printf 'test-server-check-nextest: %s\n' "$*" >&2
  exit 1
}

[ -x "$runner" ] || fail 'missing run-workspace-nextest.sh'

block=$(
  awk '/^  server-check:/{flag=1} flag && /^  [a-z].*:/ && !/^  server-check:/{exit} flag{print}' \
    "$workflow"
)
printf '%s\n' "$block" | grep -q 'tool: cargo-nextest' \
  || fail 'server-check must install cargo-nextest'
printf '%s\n' "$block" | grep -q 'run-workspace-nextest.sh' \
  || fail 'server-check must run workspace tests through run-workspace-nextest.sh'
printf '%s\n' "$block" | grep -q 'Swatinem/rust-cache@' \
  || fail 'server-check must keep Swatinem/rust-cache'

grep -q 'run-workspace-nextest.sh' "$check" \
  || fail 'check.sh must use run-workspace-nextest.sh'
grep -q 'NEXTEST_SHARDS' "$runner" \
  || fail 'run-workspace-nextest.sh must shard with NEXTEST_SHARDS'

printf 'server-check nextest guard ok\n'
