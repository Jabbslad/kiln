#!/bin/sh
# Read-only proof that the main run reuse accepted finished server-check
# successfully at this exact commit. A miss turns the reuse into a rebuild.
set -eu

output=${GITHUB_OUTPUT:-}
if [ -z "$output" ] || [ ! -f "$output" ]; then
  printf 'reuse-server-check: GITHUB_OUTPUT is missing\n' >&2
  exit 1
fi

mode=$(awk -F= '$1 == "mode" { value = $2 } END { print value }' "$output")
if [ "$mode" != reuse ]; then
  exit 0
fi

refuse() {
  message=$1
  printf 'reuse-server-check: %s\n' "$message" >&2
  awk -v message="$message" '
    BEGIN { wrote_mode = 0; wrote_reason = 0 }
    /^mode=/ { print "mode=fallback"; wrote_mode = 1; next }
    /^reason=/ { print "reason=" message; wrote_reason = 1; next }
    { print }
    END {
      if (!wrote_mode) print "mode=fallback"
      if (!wrote_reason) print "reason=" message
    }
  ' "$output" >"$output.new"
  mv "$output.new" "$output"
  exit 0
}

reason=$(awk -F= '$1 == "reason" { value = substr($0, index($0, "=") + 1) } END { print value }' "$output")
run_id=$(printf '%s\n' "$reason" | sed -n 's/^run \([0-9][0-9]*\) attestation=\([a-z-]*\)$/\1/p')
attestation=$(printf '%s\n' "$reason" | sed -n 's/^run \([0-9][0-9]*\) attestation=\([a-z-]*\)$/\2/p')
if [ "$attestation" != verified ] || [ -z "$run_id" ]; then
  refuse "attestation was not verified"
fi

repo=${GH_REPO:?}
case "$repo" in
  *[!A-Za-z0-9_./-]*) refuse "repository is not owner/name" ;;
esac
case "$repo" in
  */*) ;;
  *) refuse "repository is not owner/name" ;;
esac
case "$repo" in
  */*/*|*/|/*) refuse "repository is not owner/name" ;;
esac

event=${REUSE_EVENT:?}
if [ "$event" = workflow_dispatch ]; then
  sha=${REUSE_SHA_INPUT:?}
elif [ "$event" = push ]; then
  sha=${REUSE_SHA:?}
else
  refuse "reuse runs only for a tag push or a dry run"
fi
if ! printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{40}$'; then
  refuse "commit sha is not 40 hex characters"
fi

if ! listed=$(gh run list \
  --repo "$repo" \
  --workflow build.yml \
  --commit "$sha" \
  --branch main \
  --limit 20 \
  --json databaseId,headSha,headBranch,event,conclusion,status \
  --jq "[.[] | select(.headSha == \"$sha\" and .headBranch == \"main\" and .event == \"push\" and .conclusion == \"success\" and .status == \"completed\")] | max_by(.databaseId) | .databaseId" \
  2>/dev/null); then
  refuse "could not list workflow runs"
fi
if [ "$listed" != "$run_id" ]; then
  refuse "accepted run is not the successful main run for that commit"
fi

if ! meta=$(gh api "repos/${repo}/actions/runs/${run_id}" \
  --jq '[.head_sha, .head_branch, .event, .conclusion, .status, .path] | @tsv' \
  2>/dev/null); then
  refuse "could not read the workflow run"
fi
head_sha=$(printf '%s\n' "$meta" | cut -f1)
head_branch=$(printf '%s\n' "$meta" | cut -f2)
run_event=$(printf '%s\n' "$meta" | cut -f3)
conclusion=$(printf '%s\n' "$meta" | cut -f4)
status=$(printf '%s\n' "$meta" | cut -f5)
path=$(printf '%s\n' "$meta" | cut -f6)
if [ "$head_sha" != "$sha" ] || [ "$head_branch" != main ] || [ "$run_event" != push ] \
  || [ "$conclusion" != success ] || [ "$status" != completed ] \
  || [ "$path" != .github/workflows/build.yml ]; then
  refuse "accepted run is not a successful main push of this workflow"
fi

if ! jobs=$(gh api --paginate "repos/${repo}/actions/runs/${run_id}/jobs" \
  --jq '.jobs[] | select(.name == "server-check") | .conclusion' \
  2>/dev/null); then
  refuse "could not read the server-check job"
fi
if [ "$jobs" != success ]; then
  refuse "server-check did not succeed"
fi

printf 'server-check succeeded at %s on run %s\n' "$sha" "$run_id"
printf 'server_check=success\n' >>"$output"
