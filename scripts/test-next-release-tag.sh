#!/bin/sh
# v0.4.18 and v0.4.19 both count. The next patch is v0.4.20. An existing tag fails.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-next-release-tag: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fixture=$root/scripts/fixtures/releasable/tags.txt
got=$(sh "$root/scripts/next-release-tag.sh" < "$fixture")
[ "$got" = v0.4.20 ] || fail "next tag is ${got}"
sh "$root/scripts/next-release-tag.sh" --check v0.4.20 < "$fixture" \
  || fail 'v0.4.20 was treated as present'
if sh "$root/scripts/next-release-tag.sh" --check v0.4.19 < "$fixture" >"$work/out" 2>"$work/err"; then
  fail 'existing tag was accepted'
fi
grep -q 'refusing v0.4.19; tag already exists' "$work/err" \
  || fail "missing refusal: $(cat "$work/err")"
printf '%s\n' 'refs/tags/v0.4.19' 'refs/tags/v0.5.0' > "$work/higher"
got=$(sh "$root/scripts/next-release-tag.sh" < "$work/higher")
[ "$got" = v0.5.1 ] || fail "higher tag advanced to ${got}"
printf 'next release tag passed\n'
