#!/bin/sh
# Exit 0 when this build's commit is still the tip of main.
# Exit 2 after a skip line when main has moved on. Callers treat 2 as success
# and leave the tag, release, and deploy to the newer build.
set -eu

sha=${RELEASE_SHA:-${GITHUB_SHA:-}}
sha=$(printf '%s' "$sha" | tr 'A-F' 'a-f' | tr -d '[:space:]')
printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{40}$' || {
  printf 'release-tip-gate: release sha is not a commit\n' >&2
  exit 1
}

repo=${GH_REPO:-${GITHUB_REPOSITORY:?}}
tip=$(gh api "repos/${repo}/commits/main" --jq .sha) || {
  printf 'release-tip-gate: could not read main\n' >&2
  exit 1
}
tip=$(printf '%s' "$tip" | tr 'A-F' 'a-f' | tr -d '[:space:]')
printf '%s\n' "$tip" | grep -Eq '^[0-9a-f]{40}$' || {
  printf 'release-tip-gate: main tip is not a commit\n' >&2
  exit 1
}

if [ "$sha" = "$tip" ]; then
  exit 0
fi

printf 'release skipped: main moved to %s, the newer build releases\n' "$tip"
exit 2
