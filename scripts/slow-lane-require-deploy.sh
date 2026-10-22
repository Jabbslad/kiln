#!/bin/sh
# True when the given Build and package run finished its deploy job successfully.
set -eu

repo=${GH_REPO:?}
run_id=${1:?}

conclusion=$(gh api "repos/${repo}/actions/runs/${run_id}/jobs" --paginate \
  --jq '[.jobs[] | select(.name == "deploy" or .name == "deploy / deploy") | .conclusion] | .[0] // empty')

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  if [ "$conclusion" = success ]; then
    printf 'deploy=success\n' >>"$GITHUB_OUTPUT"
  else
    printf 'deploy=%s\n' "${conclusion:-missing}" >>"$GITHUB_OUTPUT"
  fi
fi

if [ "$conclusion" = success ]; then
  exit 0
fi

printf 'deploy job conclusion was %s\n' "${conclusion:-missing}"
