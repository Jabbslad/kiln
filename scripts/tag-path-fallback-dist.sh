#!/bin/sh
# Assemble a reuse-shaped dist locally when no green main ancestor is available.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'tag-path-fallback-dist: %s\n' "$*" >&2
  exit 1
}

from=${1:-}
dist=${2:-}
[ -n "$from" ] && [ -n "$dist" ] || fail 'usage: tag-path-fallback-dist.sh FROM_TAG DIST_DIR'
printf '%s\n' "$from" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "from-tag ${from} is not stable"
mkdir -p "$dist"

if [ -n "${TAG_PATH_FALLBACK_DIST_CMD:-}" ]; then
  sh -c "$TAG_PATH_FALLBACK_DIST_CMD"
  exit $?
fi

sh "$root/scripts/tag-path-fallback-build.sh" "$from" "$dist"

printf 'tag-path-fallback-dist: assembled dist for %s\n' "$from" >&2
