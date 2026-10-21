#!/bin/sh
# feature-map-guard job from .github/workflows/build.yml
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

base=${PR_BASE_SHA:-}
if [ -z "$base" ]; then
  git fetch origin main 2>/dev/null || true
  if git rev-parse --verify origin/main >/dev/null 2>&1; then
    base=$(git merge-base HEAD origin/main 2>/dev/null || echo origin/main)
  else
    base=origin/main
  fi
fi

git diff --name-only --no-renames "$base" HEAD \
  | sh scripts/check-feature-map.sh
