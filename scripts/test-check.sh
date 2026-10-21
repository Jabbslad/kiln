#!/bin/sh
# Guard: required CI jobs stay wired to scripts/check.sh and scripts/ci/* runners.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo test -p ci-guards local_check_scripts_stay_wired --locked -- --nocapture

printf 'check wiring tests ok\n'
