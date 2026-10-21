#!/bin/sh
# Wait until main's build.yml run for this commit has completed.
# reuse_release.py then sees a finished run. Failure, cancellation, and the
# 15 minute bound record a fallback and do not call it.
# REUSE_WAIT_LIMIT and REUSE_POLL_INTERVAL are seconds. Tests shorten them.
set -eu

limit=${REUSE_WAIT_LIMIT:-900}
interval=${REUSE_POLL_INTERVAL:-15}
case $limit in
  ''|*[!0-9]*) limit=900 ;;
esac
case $interval in
  ''|*[!0-9]*) interval=15 ;;
esac
if [ "$interval" -eq 0 ]; then
  interval=1
fi

event=${REUSE_EVENT:-}
repo=${GH_REPO:-}
case $event in
  workflow_dispatch) sha=${REUSE_SHA_INPUT:-} ;;
  push) sha=${REUSE_SHA:-} ;;
  *) exit 0 ;;
esac

printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{40}$' || exit 0
printf '%s\n' "$repo" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' || exit 0

refuse() {
  message=$1
  printf 'reuse refused: %s\n' "$message" >&2
  output=${GITHUB_OUTPUT:-}
  if [ -z "$output" ] || [ ! -f "$output" ]; then
    printf 'wait-main-run: GITHUB_OUTPUT is missing\n' >&2
    exit 1
  fi
  {
    printf 'mode=fallback\n'
    printf 'version=\n'
    printf 'reason=%s\n' "$message"
  } >>"$output"
  exit 0
}

list_query='[ .workflow_runs[]
  | select(.path == ".github/workflows/build.yml"
      and .head_branch == "main"
      and .event == "push"
      and .head_sha == env.REUSE_SHA_QUERY) ]
  | if length == 0 then ""
    else max_by(.id) | [(.id | tostring), .status, (.conclusion // "")] | @tsv
    end'

if ! row=$(REUSE_SHA_QUERY=$sha gh api "repos/${repo}/actions/runs?head_sha=${sha}&branch=main&event=push&per_page=20" --jq "$list_query"); then
  exit 0
fi
if [ -z "$row" ]; then
  exit 0
fi

id=$(printf '%s\n' "$row" | cut -f1)
status=$(printf '%s\n' "$row" | cut -f2)
conclusion=$(printf '%s\n' "$row" | cut -f3)
case $id in
  ''|*[!0-9]*) exit 0 ;;
esac

start=$(date +%s)
deadline=$((start + limit))
budget=$((limit / interval))
if [ "$budget" -lt 1 ]; then
  budget=1
fi
waited=0
polls=0

while true; do
  if [ "$status" != completed ]; then
    if [ "$waited" -eq 0 ]; then
      printf 'reuse waiting: main run %s is in progress\n' "$id" >&2
      waited=1
    fi
    polls=$((polls + 1))
    now=$(date +%s)
    if [ "$now" -ge "$deadline" ] || [ "$polls" -gt "$budget" ]; then
      refuse "timed out waiting for main run $id"
    fi
    sleep "$interval"
    if ! row=$(gh run view "$id" --repo "$repo" --json databaseId,status,conclusion,headSha,headBranch,event --jq '[.databaseId, .status, (.conclusion // ""), .headSha, .headBranch, .event] | @tsv'); then
      refuse "could not read the workflow run"
    fi
    id=$(printf '%s\n' "$row" | cut -f1)
    status=$(printf '%s\n' "$row" | cut -f2)
    conclusion=$(printf '%s\n' "$row" | cut -f3)
    head_sha=$(printf '%s\n' "$row" | cut -f4)
    head_branch=$(printf '%s\n' "$row" | cut -f5)
    run_event=$(printf '%s\n' "$row" | cut -f6)
    if [ "$head_sha" != "$sha" ] || [ "$head_branch" != main ] || [ "$run_event" != push ]; then
      refuse "main run $id is not the main push for that commit"
    fi
    continue
  fi

  if [ -z "$conclusion" ]; then
    refuse "conclusion is not a string"
  fi
  if [ "$waited" -eq 0 ]; then
    exit 0
  fi
  if [ "$conclusion" = success ]; then
    exit 0
  fi
  if [ "$conclusion" = cancelled ]; then
    refuse "main run $id was cancelled"
  fi
  refuse "main run $id failed"
done
