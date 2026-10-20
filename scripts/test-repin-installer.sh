#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
fixture=$root/scripts/fixtures/repin
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cp -R "$fixture/tree/." "$tmp/tree"
REPIN_SHA_DIR=$fixture/sha256
export REPIN_SHA_DIR
sh "$root/scripts/repin-installer.sh" pin v0.4.13 "$tmp/tree"

diff -u \
    --label a/deploy/bootstrap/install.sh \
    --label b/deploy/bootstrap/install.sh \
    "$fixture/tree/deploy/bootstrap/install.sh" \
    "$tmp/tree/deploy/bootstrap/install.sh" >"$tmp/install.diff" || test $? -eq 1
diff -u \
    --label a/docs/releases.md \
    --label b/docs/releases.md \
    "$fixture/tree/docs/releases.md" \
    "$tmp/tree/docs/releases.md" >"$tmp/docs.diff" || test $? -eq 1
cat "$tmp/install.diff" "$tmp/docs.diff" >"$tmp/got.diff"
if ! cmp -s "$fixture/expected.diff" "$tmp/got.diff"; then
    diff -u "$fixture/expected.diff" "$tmp/got.diff" >&2 || true
    printf 'test-repin-installer: pin diff mismatch\n' >&2
    exit 1
fi
printf '%s\n' ok
