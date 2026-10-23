#!/bin/sh
# Guard and proof: hung server-check steps leave teed logs and timeout evidence.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow=$root/.github/workflows/build.yml
wrapper=$root/scripts/server-check-run-test-and-lint.sh
body=$root/scripts/server-check-test-and-lint.sh
nextest=$root/.config/nextest.toml

fail() {
  printf 'test-server-check-hang-evidence: %s\n' "$*" >&2
  exit 1
}

[ -x "$wrapper" ] || fail 'missing server-check-run-test-and-lint.sh'
[ -f "$body" ] || fail 'missing server-check-test-and-lint.sh'

block=$(
  awk '/^  server-check:/{flag=1} flag && /^  [a-z].*:/ && !/^  server-check:/{exit} flag{print}' \
    "$workflow"
)
printf '%s\n' "$block" | grep -q 'server-check-run-test-and-lint.sh' \
  || fail 'server-check must run test-and-lint through server-check-run-test-and-lint.sh'
printf '%s\n' "$block" | grep -q 'timeout-minutes: 30' \
  || fail 'server-check job must cap at 30 minutes'
printf '%s\n' "$block" | grep -q 'name: server-check-test-and-lint' \
  || fail 'server-check must upload server-check-test-and-lint log artifact'
printf '%s\n' "$block" | grep -q 'if: always()' \
  || fail 'server-check log upload must use if: always()'
printf '%s\n' "$block" | grep -q 'continue-on-error: true' \
  || fail 'server-check log upload must continue-on-error: true'

grep -q 'slow-timeout' "$nextest" \
  || fail 'nextest ci profile must set slow-timeout'
grep -q 'period = "60s"' "$nextest" \
  || fail 'nextest slow-timeout period must be 60s'
grep -q 'terminate-after = 3' "$nextest" \
  || fail 'nextest slow-timeout must terminate-after 3'

grep -q 'hang_probe_sleep' "$root/crates/ci-guards/tests/hang_probe.rs" \
  || fail 'ci-guards must expose hang_probe_sleep for hang proofs'

log=$(mktemp)
export SERVER_CHECK_TEST_AND_LINT_LOG="$log"
export SERVER_CHECK_STEP_TIMEOUT_SEC=4
export KILN_SERVER_CHECK_TEST_AND_LINT_MODE=hang-probe
export KILN_SERVER_CHECK_HANG_PROBE_SECS=120

if bash "$wrapper"; then
  fail 'wrapper must exit non-zero when step timeout fires'
fi

[ -s "$log" ] || fail 'step log must be non-empty after timeout'
grep -q 'hang_probe_sleep' "$log" \
  || fail 'step log must name hang_probe_sleep after timeout'
grep -q 'server-check hang evidence' "$log" \
  || fail 'step log must include hang evidence dump marker'

printf 'server-check hang evidence ok\n'
