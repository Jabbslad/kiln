#!/bin/sh
# Guard: scripts/check.sh default path stays aligned with required fast checks.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check: %s\n' "$*" >&2
  exit 1
}

check=$root/scripts/check.sh
body=$(cat "$check")

printf '%s\n' "$body" | grep -q 'scripts/fmt.sh --check' \
  || fail 'check.sh must run fmt in check mode'
printf '%s\n' "$body" | grep -q 'cargo clippy --workspace' \
  || fail 'check.sh must run clippy'
printf '%s\n' "$body" | grep -q 'cargo test --workspace' \
  || fail 'check.sh must run workspace tests'
printf '%s\n' "$body" | grep -q 'check-feature-map.sh' \
  || fail 'check.sh must run the feature-map guard'
printf '%s\n' "$body" | grep -q '\-\-docs-check' \
  || fail 'check.sh must expose --docs-check'
printf '%s\n' "$body" | grep -q '\-\-release' \
  || fail 'check.sh must expose --release'
printf '%s\n' "$body" | grep -q 'scripts/check-docs.sh' \
  || fail 'check.sh must wire --docs-check to check-docs.sh'
printf '%s\n' "$body" | grep -q 'scripts/tag-path-dry-run.sh' \
  || fail 'check.sh must wire --release to tag-path-dry-run'
printf '%s\n' "$body" | grep -q 'shellcheck not installed, skipped' \
  || fail 'check.sh must skip shellcheck when missing'

grep -q 'sh scripts/test-check.sh' .github/workflows/build.yml \
  || fail 'build.yml must run test-check.sh'

printf 'check wiring tests ok\n'
