#!/usr/bin/env bash
# Wrap server-check test-and-lint with step timeout and tee log.
set -euo pipefail

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

log=${SERVER_CHECK_TEST_AND_LINT_LOG:-server-check-test-and-lint.log}
timeout_sec=${SERVER_CHECK_STEP_TIMEOUT_SEC:-1500}

dump_evidence() {
  sh scripts/server-check-hang-evidence-dump.sh
}

on_signal() {
  dump_evidence
}

trap on_signal TERM INT

: >"$log"

set +e
timeout --signal=TERM --kill-after=30s "$timeout_sec" \
  sh scripts/server-check-test-and-lint.sh > >(tee -a "$log") 2>&1
step_status=$?
set -e

trap - TERM INT

if [ "$step_status" -ne 0 ]; then
  dump_evidence | tee -a "$log"
  exit "$step_status"
fi

exit 0
