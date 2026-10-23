#!/bin/sh
# Hermetic budget: fixture JSON only. gh must not run when the fixture is set.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-ci-budget: %s\n' "$*" >&2
  exit 1
}

command -v jq >/dev/null 2>&1 || fail 'jq is required'
[ -f scripts/ci-budget.sh ] || fail 'missing scripts/ci-budget.sh'
[ -f scripts/fixtures/ci-budget/jobs.json ] || fail 'missing jobs fixture'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
cat >"$work/bin/gh" <<'EOF'
#!/bin/sh
printf 'gh called\n' >>"${CI_BUDGET_GH_MARK:?}"
exit 99
EOF
chmod +x "$work/bin/gh"

run_budget() {
  now=$1
  event=$2
  enforce=$3
  summary=$work/summary
  : >"$summary"
  : >"$work/gh-called"
  : >"$work/out"
  : >"$work/err"
  set +e
  (
    cd "$root"
    unset GITHUB_EVENT_NAME || true
    export PATH="$work/bin:$PATH"
    export CI_BUDGET_JOBS_JSON="$root/scripts/fixtures/ci-budget/jobs.json"
    export CI_BUDGET_NOW="$now"
    export CI_BUDGET_GH_MARK="$work/gh-called"
    export GITHUB_STEP_SUMMARY="$summary"
    export GITHUB_RUN_ID=1
    export GITHUB_REPOSITORY=Jabbslad/kiln
    if [ -n "$event" ]; then
      export GITHUB_EVENT_NAME="$event"
    fi
    if [ -n "$enforce" ]; then
      export CI_BUDGET_ENFORCE="$enforce"
    else
      unset CI_BUDGET_ENFORCE || true
    fi
    sh scripts/ci-budget.sh >"$work/out" 2>"$work/err"
  )
  status=$?
  set -e
  [ ! -s "$work/gh-called" ] || fail "gh ran for now=$now event=$event"
  [ -z "$(cat "$work/err")" ] || fail "stderr for now=$now: $(cat "$work/err")"
  return 0
}

# Earliest real start is changes at 12:00:00. The skipped job's 10:00 start
# must not move the total. tag-path is still running and must not add time past now.
run_budget 2026-10-23T12:07:00Z pull_request 1
[ "$status" -eq 0 ] || fail "under 8 min exited $status"
grep -q 'ci-budget: run 420s' "$work/out" || fail "expected 420s: $(cat "$work/out")"
grep -q 'ci-budget: changes completed 42s' "$work/out" || fail 'changes wall missing'
grep -q 'ci-budget: docs-check skipped' "$work/out" || fail 'skipped job missing'
grep -q 'ci-budget: server-check completed 345s' "$work/out" || fail 'server-check wall missing'
grep -q 'ci-budget: tag-path-dry-run in_progress 400s' "$work/out" \
  || fail 'in-progress tag-path must be measured up to now'
if grep -q '::warning::' "$work/out" || grep -q '::error::' "$work/out"; then
  fail 'under 8 min must not warn or fail'
fi
grep -q '420' "$work/summary" || fail 'summary missing run total'

run_budget 2026-10-23T12:10:00Z pull_request ''
[ "$status" -eq 0 ] || fail "8 to 12 min exited $status"
grep -q 'ci-budget: run 600s' "$work/out" || fail "expected 600s: $(cat "$work/out")"
grep -q '::warning::CI run 600s exceeds 8 minute budget' "$work/out" \
  || fail 'pull_request over 8 min must warn'
if grep -q '::error::' "$work/out"; then
  fail '8 to 12 min must not fail'
fi

run_budget 2026-10-23T12:12:00Z '' 1
[ "$status" -eq 0 ] || fail "exactly 12 min exited $status"
grep -q 'ci-budget: run 720s' "$work/out" || fail "expected 720s: $(cat "$work/out")"
grep -q '::warning::' "$work/out" || fail 'exactly 12 min must warn'
if grep -q '::error::' "$work/out"; then
  fail 'exactly 12 min must not fail'
fi

run_budget 2026-10-23T12:13:01Z '' 1
[ "$status" -eq 1 ] || fail "over 12 min exited $status"
grep -q 'ci-budget: run 781s' "$work/out" || fail "expected 781s: $(cat "$work/out")"
grep -q '::error::CI run 781s exceeds 12 minute budget' "$work/out" \
  || fail 'over 12 min must error'
grep -q 'tag-path-dry-run' "$work/out" || fail 'over-budget report dropped tag-path'

run_budget 2026-10-23T12:13:01Z push ''
[ "$status" -eq 0 ] || fail "non-PR over 12 min exited $status"
if grep -q '::error::' "$work/out" || grep -q '::warning::' "$work/out"; then
  fail 'budget enforcement is pull_request only'
fi
grep -q 'ci-budget: run 781s' "$work/out" || fail 'non-PR must still report the total'

printf 'ci budget ok\n'
