#!/bin/sh
# Smoke-test scripts/proof.sh: built dashboard preview yields a non-empty PNG.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-proof: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

export KILN_PROOF_DIR="$work/out"
export KILN_PROOF_SKIP_KILN_APP=1

sh "$root/scripts/proof.sh" dashboard --phase smoke

shot="$KILN_PROOF_DIR/smoke-dashboard-boxes.png"
[ -f "$shot" ] || fail "missing screenshot $shot"
[ -s "$shot" ] || fail "empty screenshot $shot"

# PNG magic
signature=$(head -c 8 "$shot" | od -An -tx1 | tr -d ' \n')
[ "$signature" = "89504e470d0a1a0a" ] || fail "screenshot is not a PNG"

printf 'proof smoke tests ok\n'
