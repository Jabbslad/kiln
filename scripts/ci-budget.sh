#!/bin/sh
# Report this run's job wall times. Pull requests warn above 8 minutes and
# fail above 12. Run total is now minus the earliest started job, so a job
# that is still going does not add time past now.
set -eu

warn_s=480
fail_s=720

if [ -n "${CI_BUDGET_NOW:-}" ]; then
  now=$CI_BUDGET_NOW
else
  now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
fi

if [ -n "${CI_BUDGET_JOBS_JSON:-}" ]; then
  if [ -f "$CI_BUDGET_JOBS_JSON" ]; then
    raw=$(cat "$CI_BUDGET_JOBS_JSON")
  else
    raw=$CI_BUDGET_JOBS_JSON
  fi
elif [ -n "${GITHUB_RUN_ID:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ]; then
  raw=$(gh api --paginate --slurp "repos/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}/jobs")
else
  printf 'ci-budget: set CI_BUDGET_JOBS_JSON or run inside GitHub Actions\n' >&2
  exit 1
fi

filter=$(
  cat <<'EOF'
def epoch:
  if . == null or . == "" then null
  else (tostring | sub("\\.[0-9]+"; "") | fromdateiso8601)
  end;
def jobs:
  if type == "array" and length > 0 and (.[0] | type == "object") and (.[0] | has("jobs")) then
    [ .[] | .jobs[]? ]
  elif type == "object" and has("jobs") then
    .jobs
  elif type == "array" then
    .
  else
    []
  end;
($now | epoch) as $now_e
| jobs as $jobs
| [
    $jobs[]
    | select((.conclusion != "skipped") and (.status != "skipped"))
    | .started_at
    | epoch
    | select(. != null)
  ] as $starts
| ($starts | if length == 0 then null else min end) as $min
| if $min == null then
    "ERROR\tno started job"
  else
    (
      $jobs[]
      | (.name // "unknown") as $name
      | (
          if .conclusion == "skipped" or .status == "skipped" then "skipped"
          elif .status == "completed" then "completed"
          elif .status == "in_progress" then "in_progress"
          elif .status == "queued" or .status == "waiting" or .status == "pending" then "queued"
          else (.status // "unknown")
          end
        ) as $state
      | (
          if $state == "skipped" or $state == "queued" then ""
          elif $state == "completed" and (.completed_at != null) and (.completed_at != "") and (.started_at != null) and (.started_at != "") then
            (((.completed_at | epoch) - (.started_at | epoch)) | if . < 0 then 0 else . end | floor | tostring)
          elif (.started_at != null) and (.started_at != "") then
            (($now_e - (.started_at | epoch)) | if . < 0 then 0 else . end | floor | tostring)
          else ""
          end
        ) as $wall
      | "\($name)\t\($state)\t\($wall)"
    ),
    ("TOTAL\t" + ((($now_e - $min) | if . < 0 then 0 else . end | floor | tostring)))
  end
EOF
)

report=$(printf '%s\n' "$raw" | jq -r --arg now "$now" "$filter")
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
printf '%s\n' "$report" >"$tmp"

summary=${GITHUB_STEP_SUMMARY:-}
if [ -n "$summary" ]; then
  {
    printf '## CI time budget\n\n'
    printf '| job | state | seconds |\n| --- | --- | --- |\n'
  } >>"$summary"
fi

total=
while IFS=$(printf '\t') read -r name state wall; do
  [ -n "$name" ] || continue
  if [ "$name" = ERROR ]; then
    printf 'ci-budget: %s\n' "$state" >&2
    exit 1
  fi
  if [ "$name" = TOTAL ]; then
    total=$state
    continue
  fi
  if [ -n "$wall" ]; then
    printf 'ci-budget: %s %s %ss\n' "$name" "$state" "$wall"
  else
    printf 'ci-budget: %s %s\n' "$name" "$state"
  fi
  if [ -n "$summary" ]; then
    printf '| %s | %s | %s |\n' "$name" "$state" "$wall" >>"$summary"
  fi
done <"$tmp"

if [ -z "$total" ]; then
  printf 'ci-budget: missing run total\n' >&2
  exit 1
fi

printf 'ci-budget: run %ss\n' "$total"
if [ -n "$summary" ]; then
  printf '\nRun total: %ss. Warn above %ss. Fail above %ss.\n' "$total" "$warn_s" "$fail_s" >>"$summary"
fi

enforce=0
if [ "${CI_BUDGET_ENFORCE:-}" = 1 ] || [ "${GITHUB_EVENT_NAME:-}" = pull_request ]; then
  enforce=1
fi

if [ "$enforce" -eq 1 ] && [ "$total" -gt "$fail_s" ]; then
  printf '::error::CI run %ss exceeds 12 minute budget\n' "$total"
  exit 1
fi
if [ "$enforce" -eq 1 ] && [ "$total" -gt "$warn_s" ]; then
  printf '::warning::CI run %ss exceeds 8 minute budget\n' "$total"
fi
