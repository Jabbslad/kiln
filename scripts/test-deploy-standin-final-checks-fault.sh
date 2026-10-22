#!/bin/sh
# Prove the stand-in final-check job rejects a 650f541-class Caddy miss.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-deploy-standin-final-checks-fault: %s\n' "$*" >&2
  exit 1
}

if ! command -v openssl >/dev/null 2>&1; then
  fail 'openssl is required'
fi

if ! docker info >/dev/null 2>&1 && ! command -v dockerd >/dev/null 2>&1; then
  printf 'test-deploy-standin-final-checks-fault: skipped (docker unavailable)\n'
  exit 0
fi

KILN_STANDIN_FAULT_INJECT=1 sh "$root/scripts/deploy-standin-final-checks.sh" \
  || fail 'fault injection harness failed'

printf 'test-deploy-standin-final-checks-fault: ok\n'
