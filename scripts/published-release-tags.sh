#!/bin/sh
# List stable vX.Y.Z tags that have a published GitHub release (one tag per line).
set -eu

if [ -n "${KILN_PUBLISHED_RELEASE_TAGS:-}" ]; then
  cat "$KILN_PUBLISHED_RELEASE_TAGS"
  exit 0
fi

repo=${GH_REPO:-Jabbslad/kiln}
case $repo in
  */*) ;;
  *) printf 'published-release-tags: repository is not owner/name\n' >&2; exit 1 ;;
esac

if [ -z "${GH_TOKEN:-}" ] && ! gh auth status >/dev/null 2>&1; then
  exit 1
fi

gh release list --repo "$repo" --limit 500 --json tagName --jq '.[].tagName'
