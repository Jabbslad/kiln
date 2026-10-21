#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-embedded-release-notes: %s\n' "$*" >&2
  exit 1
}

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
tag=v$raw
sh "$root/scripts/check-embedded-release-notes.sh" "$tag" || fail 'embedded notes check failed'
printf 'test-embedded-release-notes ok\n'
