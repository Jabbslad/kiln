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
printf '%s\n' "$block" | grep -q 'timeout-minutes: 30' \
  || fail 'server-check must cap job runtime at 30 minutes'
printf '%s\n' "$block" | grep -q 'Swatinem/rust-cache@' \
  || fail 'server-check must keep Swatinem/rust-cache'

nextest_config=$root/.config/nextest.toml
grep -q 'slow-timeout' "$nextest_config" \
  || fail 'nextest ci profile must set slow-timeout'
grep -q 'terminate-after' "$nextest_config" \
  || fail 'nextest slow-timeout must terminate hung tests'
grep -q 'NEXTEST_PROFILE' "$runner" \
  || fail 'run-workspace-nextest.sh must run the ci nextest profile'

grep -q 'run-workspace-nextest.sh' "$check" \
  || fail 'check.sh must use run-workspace-nextest.sh'
grep -q 'NEXTEST_SHARDS' "$runner" \
  || fail 'run-workspace-nextest.sh must shard with NEXTEST_SHARDS'

printf 'server-check nextest guard ok\n'
