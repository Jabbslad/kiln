#!/bin/sh
# After a green main deploy, merge at most one oldest open slow-lane pull request,
# then dispatch Build and package on main so release and deploy run again.
set -eu

fail() {
  printf 'slow-lane-merge: %s\n' "$*" >&2
  exit 1
}

summary() {
  reason=$1
  detail=$2
  printf 'skip (%s): %s\n' "$reason" "$detail"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      printf '%s\n' '### Slow-lane merge' ''
      printf '%s\n' "- **skip (${reason}):** ${detail}"
    } >>"$GITHUB_STEP_SUMMARY"
  fi
}

merged_summary() {
  number=$1
  head=$2
  printf 'merged pull request #%s at %s\n' "$number" "$head"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      printf '%s\n' '### Slow-lane merge' ''
      printf '%s\n' "- **merged:** #${number} at \`${head}\`"
      printf '%s\n' '- **dispatched:** Build and package on main'
    } >>"$GITHUB_STEP_SUMMARY"
  fi
}

repo=${GH_REPO:?}
label=slow-lane
workflow_name=${SLOW_LANE_BUILD_WORKFLOW:-Build and package}

ensure_label() {
  if gh label list --limit 100 --json name --jq 'any(.[]; .name == "'"$label"'")' | grep -qx true; then
    return 0
  fi
  gh label create "$label" --description 'Merge after the previous main deploy is green' --color 0E8A16
}

pick_candidate() {
  gh pr list \
    --label "$label" \
    --state open \
    --json number,createdAt,isDraft,headRefOid \
    --limit 100 \
    --jq '
      [.[] | select(.isDraft | not)]
      | sort_by(.createdAt)
      | .[0]
      | select(. != null)
      | "\(.number)\t\(.headRefOid)"
    '
}

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
    [.labels[].name]
    | any(. == "hold" or . == "gated")
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
    | if length == 0 then "ok"
      elif any(. == "pending") then "pending"
      elif any(. == "bad") then "bad"
      else "ok"
      end
  '
}

level_with_main() {
  head=$1
  behind=$(gh api "repos/${repo}/compare/main...${head}" --jq .behind_by)
  [ "${behind:-}" = 0 ]
}

diff_has_py() {
  number=$1
  gh pr diff "$number" --name-only | grep -q '\.py$'
}

merge_squash() {
  number=$1
  head=$2
  gh pr merge "$number" --squash --match-head-commit "$head"
  gh pr view "$number" --json merged --jq .merged | grep -qx true
}

dispatch_build() {
  gh workflow run "$workflow_name" --ref main
}

ensure_label || fail 'could not ensure slow-lane label'

line=$(pick_candidate || true)
if [ -z "${line:-}" ]; then
  summary none 'no open non-draft pull request labelled slow-lane'
  exit 0
fi

number=${line%%	*}
head=${line#*	}

body=$(gh pr view "$number" --json body --jq .body)
if body_blocks_merge "$body"; then
  summary body "#${number} body contains do not merge"
  exit 0
fi

if pr_has_block_label "$number"; then
  summary label "#${number} has hold or gated label"
  exit 0
fi

verdict=$(checks_verdict "$head")
case $verdict in
  ok) ;;
  pending)
    summary checks "#${number} has pending check runs at ${head}"
    exit 0
    ;;
  *)
    summary checks "#${number} check runs are not all success or skipped at ${head}"
    exit 0
    ;;
esac

if ! level_with_main "$head"; then
  summary behind "#${number} is behind main (compare main...${head})"
  exit 0
fi

if diff_has_py "$number"; then
  summary python "#${number} diff includes a .py file"
  exit 0
fi

merge_squash "$number" "$head" || fail "merge failed for #${number}"
dispatch_build || fail 'workflow dispatch failed after merge'
merged_summary "$number" "$head"
