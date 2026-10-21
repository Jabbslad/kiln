#!/bin/sh
# release.json version reads must use typed JSON, not fragile sed backrefs.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
reader=$root/scripts/read-release-json-field.sh

fail() {
  printf 'test-release-json-field: %s\n' "$*" >&2
  exit 1
}

json='{"min_upgrade_from":"0.3.0","target":"x86_64-unknown-linux-gnu","version":"v0.4.40"}'
got=$(printf '%s\n' "$json" | sh "$reader" version)
[ "$got" = v0.4.40 ] || fail "expected v0.4.40, got ${got}"
bare=$(printf '%s\n' "$json" | sh "$reader" version --strip-v)
[ "$bare" = 0.4.40 ] || fail "expected 0.4.40 stripped, got ${bare}"

# A broken sed backreference (\2 with one capture) used to yield min_upgrade_from.
broken='sed '"'"'s/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\2/'"'"''
trap=$(printf '%s\n' "$json" | sh -c "$broken" 2>/dev/null || true)
[ "$trap" != 0.4.40 ] || fail 'fixture must not match the old sed backreference bug'

if printf '%s\n' "$json" | sh "$reader" min_upgrade_from --strip-v | grep -q '^0.3.0$'; then
  :
else
  fail 'min_upgrade_from must remain addressable separately from version'
fi

if printf '[]\n' | sh "$reader" version 2>/dev/null; then
  fail 'non-object JSON must fail'
fi

cargo xtask test-release-version >/dev/null || fail 'xtask release-version tests failed'

printf 'release.json field read ok\n'
