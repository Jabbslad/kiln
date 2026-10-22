#!/bin/sh
# Write web/src/release-notes.json for the dashboard build.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'write-release-notes: %s\n' "$*" >&2
  exit 1
}

if [ -n "${VERSION:-}" ]; then
  tag=$VERSION
else
  raw=$(awk '
    /^\[workspace\.package\]/ { in_block = 1; next }
    /^\[/ { in_block = 0 }
    in_block && /^version = "/ {
      line = $0
      sub(/^version = "/, "", line)
      sub(/".*/, "", line)
      print line
      exit
    }
  ' Cargo.toml)
  printf '%s\n' "$raw" | grep -Eq '^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
    || fail "workspace version ${raw:-empty}"
  tag=v$raw
fi

printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "version ${tag} is not stable"

sh "$root/scripts/release-notes-json.sh" "$tag" >"$root/web/src/release-notes.json"
