#!/bin/sh
# Merge ready pull requests in stack order, then dispatch Build once on main.
set -eu

fail() {
  printf 'batch-merge: %s\n' "$*" >&2
  exit 1
}

summary_line() {
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s\n' "$1" >>"$GITHUB_STEP_SUMMARY"
  fi
}

summary_header() {
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      printf '%s\n' '### Batch merge' ''
    } >>"$GITHUB_STEP_SUMMARY"
  fi
}

repo=${GH_REPO:?}
workflow_name=${BATCH_BUILD_WORKFLOW:-Build and package}
workflow_file=${BATCH_BUILD_WORKFLOW_FILE:-build.yml}
dry_run=0
case ${BATCH_MERGE_DRY_RUN:-false} in
  true | True | 1 | yes) dry_run=1 ;;
esac
prs_cut=${BATCH_MERGE_PRS:-}

body_blocks_merge() {
  body=$1
  while IFS= read -r line || [ -n "$line" ]; do
    if printf '%s\n' "$line" | grep -qiE 'do not merge|don'\''t merge'; then
      return 0
    fi
  done <<EOF
$body
EOF
  return 1
}

pr_has_block_label() {
  number=$1
  gh pr view "$number" --json labels --jq '
    [.labels[].name | ascii_downcase]
    | any(. == "do-not-merge" or . == "hold" or . == "gated")
  ' | grep -qx true
}

checks_verdict() {
  head=$1
  gh api "repos/${repo}/commits/${head}/check-runs" --paginate --jq '
    [.check_runs[]
      | if .status != "completed" then "pending"
        elif (.conclusion == "success" or .conclusion == "skipped") then "ok"
        else "bad"
        end]
    | if length == 0 then "missing"
      elif any(. == "pending") then "pending"
      elif any(. == "bad") then "bad"
      else "ok"
      end
  '
}

diff_has_py() {
  number=$1
  gh pr diff "$number" --name-only | grep -q '\.py$'
}

pr_mergeable() {
  number=$1
  gh pr view "$number" --json mergeable --jq .mergeable | grep -qx MERGEABLE
}

skip_comment() {
  number=$1
  reason=$2
  if [ "$dry_run" = 1 ]; then
    return 0
  fi
  gh pr comment "$number" --body "batch-merge skipped this pull request: ${reason}" >/dev/null 2>&1 \
    || printf 'batch-merge: could not comment on #%s\n' "$number" >&2
}

record_skip() {
  number=$1
  reason=$2
  printf 'skip #%s: %s\n' "$number" "$reason"
  summary_line "- **skip #${number}:** ${reason}"
  skip_comment "$number" "$reason"
}

merge_squash() {
  number=$1
  head=$2
  gh pr merge "$number" --squash --match-head-commit "$head"
  gh pr view "$number" --json merged --jq .merged | grep -qx true
}

merge_commit_sha() {
  number=$1
  gh pr view "$number" --json mergeCommit --jq '.mergeCommit.oid // empty'
}

ready_prs_json() {
  gh pr list \
    --state open \
    --json number,createdAt,isDraft,headRefOid,headRefName,baseRefName \
    --limit 100 \
    --jq '
      [.[] | select(.isDraft | not)]
      | sort_by(.createdAt)
    '
}

apply_prs_cut() {
  json=$1
  cut=$2
  if [ -z "${cut:-}" ]; then
    printf '%s' "$json"
    return 0
  fi
  printf '%s' "$json" | jq --arg cut "$cut" '
    ($cut | split(" ") | map(select(length > 0) | tonumber)) as $want |
    [.[] | select(.number as $n | ($want | index($n)))]
  '
}

stack_order_numbers() {
  prs_json=$1
  printf '%s' "$prs_json" | jq -r '
    . as $all |
    def peel($pending):
      ($pending | map(.headRefName)) as $heads |
      [ $pending[]
        | . as $pr
        | select($pr.baseRefName == "main" or (($heads | index($pr.baseRefName)) | not))
        | $pr.number
      ];
    reduce range(0; ($all | length) + 1) as $_
      ({pending: $all, order: []};
       if (.pending | length) == 0 then .
       else (peel(.pending)) as $layer |
         if ($layer | length) == 0 then error("cycle in pull request stack")
         else .order |= . + $layer
         | .pending |= map(select(.number as $n | ($layer | index($n) | not)))
         end
       end) | .order[]'
}

readiness_reason() {
  number=$1
  head=$2

  body=$(gh pr view "$number" --json body --jq .body)
  if body_blocks_merge "$body"; then
    printf '%s\n' 'body contains do not merge'
    return 0
  fi

  if pr_has_block_label "$number"; then
    printf '%s\n' 'blocking label (do-not-merge, hold, or gated)'
    return 0
  fi

  verdict=$(checks_verdict "$head")
  case $verdict in
    ok) ;;
    missing)
      printf 'no check runs at %s\n' "$head"
      return 0
      ;;
    pending)
      printf 'pending check runs at %s\n' "$head"
      return 0
      ;;
    *)
      printf 'check runs not all success or skipped at %s\n' "$head"
      return 0
      ;;
  esac

  if diff_has_py "$number"; then
    printf '%s\n' 'diff includes a .py file'
    return 0
  fi

  if ! pr_mergeable "$number"; then
    printf '%s\n' 'not mergeable (conflict or unknown)'
    return 0
  fi

  return 1
}

dispatch_build() {
  base_sha=$1
  if [ -n "$base_sha" ]; then
    gh workflow run "$workflow_name" --ref main -f "batch_base_sha=${base_sha}"
  else
    gh workflow run "$workflow_name" --ref main
  fi
}

find_dispatched_build_url() {
  poll_limit=${BATCH_MERGE_DISPATCH_POLLS:-12}
  poll_sleep=${BATCH_MERGE_DISPATCH_SLEEP:-5}
  attempts=0
  while [ "$attempts" -lt "$poll_limit" ]; do
    url=$(gh run list --repo "$repo" --workflow "$workflow_file" --branch main --limit 5 \
      --json url,event,createdAt --jq '
        [.[] | select(.event == "workflow_dispatch")] | .[0].url // empty' 2>/dev/null || true)
    if [ -n "$url" ]; then
      printf '%s\n' "$url"
      return 0
    fi
    attempts=$((attempts + 1))
    sleep "$poll_sleep"
  done
  return 1
}

main_before=$(gh api "repos/${repo}/commits/main" --jq .sha)
printf '%s\n' "$main_before" | grep -Eq '^[0-9a-f]{40}$' || fail 'could not read main tip'

open_json=$(ready_prs_json)
candidate_json=$(apply_prs_cut "$open_json" "$prs_cut")

summary_header
if [ -n "${prs_cut:-}" ]; then
  summary_line "- **cut list:** ${prs_cut}"
fi
if [ "$dry_run" = 1 ]; then
  summary_line '- **mode:** dry run (no merges, no Build dispatch)'
fi

if [ -n "${prs_cut:-}" ]; then
  for n in $prs_cut; do
    case $n in
      '' | *[!0-9]*) fail "invalid pull request number in cut list: $n" ;;
    esac
    if ! printf '%s' "$open_json" | jq -e --argjson num "$n" 'any(.[]; .number == $num)' >/dev/null; then
      record_skip "$n" 'not an open non-draft pull request'
    fi
  done
fi

if [ "$(printf '%s' "$candidate_json" | jq 'length')" -eq 0 ]; then
  if [ -z "${prs_cut:-}" ]; then
    printf 'skip (none): no open non-draft pull requests\n'
    summary_line '- **skip (none):** no open non-draft pull requests'
  else
    printf 'skip (none): no cut-list pull requests are open\n'
    summary_line '- **skip (none):** no cut-list pull requests are open'
  fi
  exit 0
fi

order=$(stack_order_numbers "$candidate_json") || fail 'could not order pull request stack'
summary_line "- **merge order:** $(printf '%s\n' $order | tr '\n' ' ' | sed 's/ $//')"

merged_count=0
for number in $order; do
  head=$(printf '%s' "$candidate_json" | jq -r --argjson n "$number" '.[] | select(.number == $n) | .headRefOid')
  [ -n "$head" ] || continue

  reason=$(readiness_reason "$number" "$head" || true)
  if [ -n "$reason" ]; then
    record_skip "$number" "$reason"
    continue
  fi

  if [ "$dry_run" = 1 ]; then
    printf 'dry-run merge #%s at %s\n' "$number" "$head"
    summary_line "- **would merge:** #${number} at \`${head}\`"
    merged_count=$((merged_count + 1))
    continue
  fi

  if ! merge_squash "$number" "$head"; then
    record_skip "$number" 'merge failed (conflict or head moved)'
    continue
  fi

  squash_sha=$(merge_commit_sha "$number")
  merged_count=$((merged_count + 1))
  if [ -n "$squash_sha" ]; then
    printf 'merged pull request #%s at %s (squash %s)\n' "$number" "$head" "$squash_sha"
    summary_line "- **merged:** #${number} squash \`${squash_sha}\` (matched head \`${head}\`)"
  else
    printf 'merged pull request #%s at %s\n' "$number" "$head"
    summary_line "- **merged:** #${number} at \`${head}\`"
  fi
done

if [ "$merged_count" -eq 0 ]; then
  printf 'skip (none): no pull requests merged\n'
  summary_line '- **skip (none):** no pull requests merged'
  exit 0
fi

if [ "$dry_run" = 1 ]; then
  printf 'dry-run: would dispatch Build and package on main (batch_base_sha=%s)\n' "$main_before"
  summary_line "- **would dispatch:** Build and package on main (\`batch_base_sha=${main_before}\`)"
  exit 0
fi

dispatch_build "$main_before" || fail 'workflow dispatch failed after merge'
if build_url=$(find_dispatched_build_url); then
  summary_line "- **dispatched:** [Build and package](${build_url})"
  printf 'dispatched Build and package: %s (batch_base_sha=%s)\n' "$build_url" "$main_before"
else
  summary_line '- **dispatched:** Build and package on main (run URL not found yet)'
  printf 'dispatched Build and package on main (batch_base_sha=%s)\n' "$main_before"
fi
