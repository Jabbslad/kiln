#!/bin/sh
# Rule 3: pull requests that touch guarded product paths must update the verify-kiln
# feature map or declare feature-map: n/a in the PR body.
set -eu

fail() {
  printf 'feature-map-guard: %s\n' "$*" >&2
  exit 1
}

guarded=0
features=0

path_guarded() {
  case "$1" in
    web/src/*|deploy/*|.github/workflows/*) return 0 ;;
    crates/*/src/*) return 0 ;;
    *) return 1 ;;
  esac
}

path_features() {
  case "$1" in
    .cursor/skills/verify-kiln/features/*) return 0 ;;
    *) return 1 ;;
  esac
}

while IFS= read -r path || [ -n "$path" ]; do
  [ -n "$path" ] || continue
  if path_guarded "$path"; then
    guarded=1
  fi
  if path_features "$path"; then
    features=1
  fi
done

if [ "$guarded" -eq 0 ]; then
  exit 0
fi

if [ "$features" -eq 1 ]; then
  exit 0
fi

body=${PR_BODY:-}
if printf '%s\n' "$body" | grep -Fxq 'feature-map: n/a'; then
  exit 0
fi

fail 'guarded paths changed without updating .cursor/skills/verify-kiln/features/ (or add a PR body line: feature-map: n/a)'
