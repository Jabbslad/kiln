#!/bin/sh
# Decide whether this main push is a release, and name the next patch tag.
# PLAN_RELEASE_APPLY=1 creates that tag with the workflow token. A tag push
# does not start a second workflow, so the same run still publishes and deploys.
# The tag is created before this script returns. The next queued release sees it.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'plan-release: %s\n' "$*" >&2
  exit 1
}

emit() {
  printf '%s\n' "$1"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf '%s\n' "$1" >> "$GITHUB_OUTPUT"
  fi
}

from_version() {
  awk '
    /^\[workspace\.package\]/ { in_block = 1; next }
    /^\[/ { in_block = 0 }
    in_block && /^version = "/ {
      line = $0
      sub(/^version = "/, "", line)
      sub(/".*/, "", line)
      print line
      exit
    }
  ' Cargo.toml
}

from_raw=$(from_version)
printf '%s\n' "$from_raw" | grep -Eq '^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "workspace version ${from_raw:-empty}"
from=v$from_raw

if [ -n "${PLAN_RELEASE_PATHS:-}" ]; then
  decision=$(sh "$root/scripts/releasable.sh" < "$PLAN_RELEASE_PATHS")
elif [ "${BEFORE:-}" = 0000000000000000000000000000000000000000 ]; then
  decision=release=true
else
  before=${BEFORE:?}
  after=${AFTER:?}
  paths=$(git diff --name-only --no-renames "$before" "$after") || fail 'git diff failed'
  decision=$(printf '%s\n' "$paths" | sh "$root/scripts/releasable.sh")
fi
[ "$decision" = release=true ] || [ "$decision" = release=false ] || fail "classifier ${decision}"

if [ "$decision" != release=true ]; then
  emit 'release=false'
  emit 'tag='
  emit "from=${from}"
  exit 0
fi

if [ -n "${PLAN_RELEASE_TAGS:-}" ]; then
  tags=$(cat "$PLAN_RELEASE_TAGS")
else
  tags=$(git ls-remote --tags origin 'refs/tags/v*') || fail 'could not list tags'
fi
tag=$(printf '%s\n' "$tags" | sh "$root/scripts/next-release-tag.sh")
printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "next tag ${tag}"
if ! printf '%s\n' "$tags" | sh "$root/scripts/next-release-tag.sh" --check "$tag"; then
  fail "refusing ${tag}; tag already exists"
fi

sha=${PLAN_RELEASE_SHA:-${GITHUB_SHA:-}}
printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{40}$' || fail 'release sha is not a commit'
apply=${PLAN_RELEASE_APPLY:-0}
if [ "$apply" = 1 ]; then
  repo=${GH_REPO:?}
  [ -n "${GH_TOKEN:-}" ] || fail 'GH_TOKEN is required'
  if ! gh api --method POST "repos/${repo}/git/refs" -f ref="refs/tags/${tag}" -f sha="$sha" >/dev/null; then
    fail "refusing ${tag}; tag already exists"
  fi
  printf 'created tag %s\n' "$tag"
else
  printf 'dry-run tag %s sha %s\n' "$tag" "$sha"
  printf 'dry-run release %s\n' "$tag"
fi

emit 'release=true'
emit "tag=${tag}"
emit "from=${from}"
