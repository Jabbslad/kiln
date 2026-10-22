#!/bin/sh
# Resolve the git range start tag for release notes: skip unpublished intermediate tags.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
repo_root=${KILN_RELEASE_NOTES_ROOT:-$root}
cd "$repo_root"

fail() {
  printf 'previous-release-notes-from: %s\n' "$*" >&2
  exit 1
}

tag=${1:-}
[ -n "$tag" ] || fail 'usage: previous-release-notes-from.sh TAG'

from=
if ! from=$(git tag -l 'v*' | sh "$root/scripts/previous-release-tag.sh" "$tag" 2>/dev/null); then
  exit 1
fi

use_published=0
if [ -n "${KILN_PUBLISHED_RELEASE_TAGS:-}" ] || [ "${KILN_RELEASE_NOTES_PUBLISHED_BASE:-0}" = 1 ]; then
  use_published=1
fi

published=
if [ "$use_published" -eq 1 ]; then
  if [ -n "${KILN_PUBLISHED_RELEASE_TAGS:-}" ]; then
    published=$(cat "$KILN_PUBLISHED_RELEASE_TAGS") || fail "could not read ${KILN_PUBLISHED_RELEASE_TAGS}"
  elif ! published=$(sh "$root/scripts/published-release-tags.sh" 2>/dev/null); then
    published=
  fi
fi

if [ -z "$published" ]; then
  printf '%s\n' "$from"
  exit 0
fi

while [ -n "$from" ] && ! printf '%s\n' "$published" | grep -Fxq "$from"; do
  next=
  if ! next=$(git tag -l 'v*' | sh "$root/scripts/previous-release-tag.sh" "$from" 2>/dev/null); then
    fail "no published release tag before ${tag}"
  fi
  from=$next
done

[ -n "$from" ] || fail "no published release tag before ${tag}"
printf '%s\n' "$from"
