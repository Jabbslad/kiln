#!/bin/sh
# PR tag-path rehearsal must stamp workspace versions before building release binaries
# and must pass the v-prefixed tag to check-binary.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
script=$root/scripts/tag-path-dry-run.sh

fail() {
  printf 'test-tag-path-pr-stamp-order: %s\n' "$*" >&2
  exit 1
}

[ -f "$script" ] || fail 'missing tag-path-dry-run.sh'

resolve_line=$(
  grep -n "cargo xtask release-version resolve --ref-type tag --ref-name \"\$tag\"" "$script" \
    | awk -F: 'NR == 1 { print $1 }'
)
build_line=$(
  grep -n 'cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client' "$script" \
    | awk -F: 'NR == 1 { print $1 }'
)
check_line=$(
  grep -n "cargo xtask release-version check-binary --version \"\$tag\"" "$script" \
    | awk -F: 'NR == 1 { print $1 }'
)

[ -n "$resolve_line" ] || fail 'missing release-version resolve for PR rehearsal'
[ -n "$build_line" ] || fail 'missing PR rehearsal cargo build'
[ -n "$check_line" ] || fail 'missing check-binary with v-prefixed tag'

if [ "$resolve_line" -ge "$build_line" ]; then
  fail 'release-version resolve must run before cargo build in PR rehearsal'
fi
if [ "$build_line" -ge "$check_line" ]; then
  fail 'cargo build must run before check-binary in PR rehearsal'
fi

if grep -q "check-binary --version \"\${tag#v}\"" "$script"; then
  fail 'check-binary must not strip the v prefix from the tag'
fi
if grep -q "check-binary --version \"\$VERSION\"" "$script"; then
  fail 'check-binary must use the v-prefixed tag, not bare VERSION'
fi

printf 'tag-path PR stamp order ok\n'
