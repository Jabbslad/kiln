#!/bin/sh
# Binary swap failures must not hide install step diagnostics behind a bare "(command)" die.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
install=$root/deploy/install.sh

fail() {
  printf 'test-install-upgrade-failure-message: %s\n' "$*" >&2
  exit 1
}

grep -q 'install step details are on stderr' "$install" \
  || fail 'upgrade failure die must point operators at install step stderr'
grep -q 'update failed (command)' "$install" \
  && fail 'upgrade failure must not use the opaque "(command)" die text'

grep -q 'report_install_step' "$install" \
  || fail 'install.sh must define report_install_step for structured failures'
grep -q 'install step failed:' "$install" \
  || fail 'install step failures must name the step'

printf 'install upgrade failure message guard ok\n'
