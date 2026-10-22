#!/bin/sh
# Exit 0 when no Build and package or Deploy workflow is in progress on main.
set -eu

repo=${GH_REPO:?}

busy=$(gh api "repos/${repo}/actions/runs?status=in_progress&per_page=100" --paginate --jq '
  [.workflow_runs[]
    | select(.head_branch == "main")
    | select(.path == ".github/workflows/build.yml" or .path == ".github/workflows/deploy.yml")
  ] | length')

if [ "${busy:-0}" -gt 0 ]; then
  printf 'batch-merge-idle: %s main build/deploy run(s) in progress\n' "$busy" >&2
  exit 1
fi
