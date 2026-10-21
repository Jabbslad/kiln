#!/bin/sh
# v0.4.19 tag push 09:01:34Z to deploy green 09:07:45Z was 371s
# (run 39573753819). The tail after packages exist is what a main auto-release pays.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-release-budget: %s\n' "$*" >&2
  exit 1
}

workflow=$root/.github/workflows/build.yml
remote=$root/deploy/github/install-remote.sh
[ -f "$workflow" ] || fail 'missing build workflow'
[ -f "$remote" ] || fail 'missing install-remote.sh'

job_body() {
  awk -v name="$1" '
    $0 == "  " name ":" { capture = 1; next }
    capture && /^  [^ ]/ { exit }
    capture { print }
  ' "$workflow"
}

# Measured stages, seconds:
# reuse 94, version-guard 80 of which cargo check-binary was 31,
# draft-release 90, deploy 94, gaps folded into those job bounds.
# Same-run publishing drops reuse. The version-guard cargo compile is gone.
# 371 - 94 - 31 = 246.
cost=246
guard=$(job_body version-guard)
draft=$(job_body draft-release)
if printf '%s\n' "$guard" | grep -q 'cargo xtask'; then
  cost=$((cost + 31))
fi
if printf '%s\n' "$guard" | grep -q 'reuse'; then
  cost=$((cost + 94))
fi
calls=$(grep -c '^run_identity$' "$remote" || true)
if [ "$calls" -gt 1 ]; then
  cost=$((cost + 1))
fi
if printf '%s\n' "$draft" | grep 'gh attestation verify' | grep -v '&' >/dev/null; then
  cost=$((cost + 15))
fi
limit=300
printf 'release budget before=371 after=%s limit=%s\n' "$cost" "$limit"
printf 'release budget stages reuse=94 version_guard_cargo=31 deploy_second_identity=1 attest_serial=15\n'
[ "$cost" -lt "$limit" ] || fail "critical path ${cost}s regresses past ${limit}s"
printf 'release budget passed\n'
