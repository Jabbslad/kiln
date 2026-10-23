#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
merge=$root/scripts/batch-merge.sh
idle_gate=$root/scripts/batch-merge-idle-gate.sh

fail() {
  printf 'test-batch-merge: %s\n' "$*" >&2
  exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

write_stub() {
  cat >"$tmp/gh" <<'STUB'
#!/bin/sh
set -eu
root=${GH_STUB_ROOT:?}
log=$root/gh.log
printf '%s\n' "$*" >>"$log"

pr_for_head() {
  head=$1
  for f in "$root"/pr_*_head; do
    [ -f "$f" ] || continue
    if [ "$(cat "$f")" = "$head" ]; then
      basename "$f" | sed 's/pr_\(.*\)_head/\1/'
      return 0
    fi
  done
  return 1
}

case "$1" in
  pr)
    shift
    case "$1" in
      list)
        shift
        jq_filter=
        while [ $# -gt 0 ]; do
          case "$1" in
            --jq) jq_filter=$2; shift 2 ;;
            *) shift ;;
          esac
        done
        jq -r "$jq_filter" "$root/pr_list.json"
        ;;
      view)
        n=$2
        shift 2
        jq_filter=
        field=
        while [ $# -gt 0 ]; do
          case "$1" in
            --json) field=$2; shift 2 ;;
            --jq) jq_filter=$2; shift 2 ;;
            *) shift ;;
          esac
        done
        case $field in
          body)
            jq -n --rawfile b "$root/pr_${n}_body.txt" '{body: $b}' | jq -r "$jq_filter"
            ;;
          labels)
            jq -r "$jq_filter" "$root/pr_${n}_labels.json"
            ;;
          merged)
            if [ -f "$root/pr_${n}_merged_after" ]; then
              jq -n '{merged: true}' | jq -r "$jq_filter"
            else
              jq -r "$jq_filter" "$root/pr_${n}_merged.json"
            fi
            ;;
          mergeable)
            if [ -f "$root/pr_${n}_mergeable" ]; then
              jq -n --arg m "$(cat "$root/pr_${n}_mergeable")" '{mergeable: $m}' | jq -r "$jq_filter"
            else
              jq -n '{mergeable: "MERGEABLE"}' | jq -r "$jq_filter"
            fi
            ;;
          mergeCommit)
            if [ -f "$root/pr_${n}_squash_sha" ]; then
              jq -n --arg s "$(cat "$root/pr_${n}_squash_sha")" '{mergeCommit: {oid: $s}}' | jq -r "$jq_filter"
            else
              jq -n '{mergeCommit: null}' | jq -r "$jq_filter"
            fi
            ;;
          *) exit 1 ;;
        esac
        ;;
      diff)
        n=$2
        cat "$root/pr_${n}_diff.txt"
        ;;
      merge)
        n=$2
        shift 2
        head=
        while [ $# -gt 0 ]; do
          case "$1" in
            --squash) ;;
            --match-head-commit) head=$2; shift ;;
            *) exit 1 ;;
          esac
          shift
        done
        printf '%s\n' "$n" >>"$root/merge_requests"
        printf '%s\n' "$head" >>"$root/merge_heads"
        if [ -f "$root/pr_${n}_merge_fail" ]; then
          exit 1
        fi
        expected=$(cat "$root/pr_${n}_head")
        if [ "$head" != "$expected" ]; then
          exit 1
        fi
        printf 'true\n' >"$root/pr_${n}_merged_after"
        printf '%040d\n' "$n" >"$root/pr_${n}_squash_sha"
        if [ -f "$root/main_after" ]; then
          cat "$root/main_after" >"$root/main_sha"
        else
          cat "$root/pr_${n}_squash_sha" >"$root/main_sha"
        fi
        ;;
      comment)
        shift
        n=$1
        shift
        while [ $# -gt 0 ]; do
          case "$1" in
            --body) body=$2; shift 2 ;;
            *) shift ;;
          esac
        done
        printf '%s\n' "$body" >>"$root/pr_${n}_comments.txt"
        ;;
      *) exit 1 ;;
    esac
    ;;
  api)
    shift
    path=$1
    shift
    jq_filter=
    while [ $# -gt 0 ]; do
      case "$1" in
        --paginate) shift ;;
        --jq) jq_filter=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    case $path in
      */commits/main)
        payload=$(jq -n --arg s "$(cat "$root/main_sha")" '{sha: $s}')
        ;;
      */actions/runs/*/jobs*)
        mode=${DEPLOY_STUB_MODE:-success}
        case $mode in
          success)
            payload=$(jq -n '{jobs: [{name: "deploy", conclusion: "success"}]}')
            ;;
          failure)
            payload=$(jq -n '{jobs: [{name: "deploy", conclusion: "failure"}]}')
            ;;
          missing)
            payload=$(jq -n '{jobs: [{name: "server-check", conclusion: "success"}]}')
            ;;
          *) exit 1 ;;
        esac
        ;;
      */check-runs)
        head=${path#*/commits/}
        head=${head%/check-runs}
        n=$(pr_for_head "$head" || true)
        if [ -n "$n" ] && [ -f "$root/pr_${n}_checks.json" ]; then
          payload=$(jq -n --slurpfile runs "$root/pr_${n}_checks.json" '{check_runs: $runs[0]}')
        else
          payload=$(jq -n '{check_runs: []}')
        fi
        ;;
      */actions/runs*)
        mode=${IDLE_STUB_MODE:-idle}
        case $mode in
          idle)
            payload=$(jq -n '{workflow_runs: []}')
            ;;
          busy)
            payload=$(jq -n '{workflow_runs: [{head_branch: "main", path: ".github/workflows/build.yml", status: "in_progress"}]}')
            ;;
          *) exit 1 ;;
        esac
        ;;
      *) exit 1 ;;
    esac
    if [ -n "$jq_filter" ]; then
      printf '%s\n' "$payload" | jq -r "$jq_filter"
    else
      printf '%s\n' "$payload"
    fi
    ;;
  workflow)
    [ "$2" = run ] || exit 1
    shift 2
    printf '%s\n' "$*" >>"$root/dispatch_args"
    printf '1\n' >>"$root/dispatch_count"
    printf 'https://github.com/example/actions/runs/999001\n' >"$root/dispatch_run_url"
    ;;
  run)
    shift
    [ "$1" = list ] || exit 1
    shift
    jq_filter=
    while [ $# -gt 0 ]; do
      case "$1" in
        --json) jq_filter=$2; shift 2 ;;
        --workflow) shift 2 ;;
        --branch) shift 2 ;;
        --repo) shift 2 ;;
        --limit) shift 2 ;;
        *) shift ;;
      esac
    done
    payload=$(jq -n --arg url "$(cat "$root/dispatch_run_url" 2>/dev/null || echo '')" \
      '[{url: $url, event: "workflow_dispatch", createdAt: "2026-01-01T00:00:00Z"}]')
    printf '%s\n' "$payload" | jq -r "$jq_filter"
    ;;
  *) exit 1 ;;
esac
STUB
  chmod 755 "$tmp/gh"
}

reset_prs() {
  printf '[]' >"$tmp/pr_list.json"
  rm -f "$tmp"/pr_*_body.txt "$tmp"/pr_*_labels.json "$tmp"/pr_*_checks.json \
    "$tmp"/pr_*_diff.txt "$tmp"/pr_*_merged.json \
    "$tmp"/pr_*_head "$tmp"/pr_*_merged_after "$tmp"/pr_*_mergeable \
    "$tmp"/pr_*_merge_fail "$tmp"/pr_*_comments.txt
}

setup_pr() {
  n=$1
  head=$2
  created=$3
  draft=${4:-false}
  body=${5:-ok}
  labels=${6:-'{"labels":[]}'}
  checks=${7:-'[{"status":"completed","conclusion":"success"}]'}
  diff=${8:-README.md}
  base=${9:-main}
  branch=${10:-branch-$n}
  mergeable=${11:-MERGEABLE}

  if [ ! -f "$tmp/pr_list.json" ]; then
    printf '[]' >"$tmp/pr_list.json"
  fi
  jq --argjson n "$n" --arg c "$created" --argjson d "$draft" --arg h "$head" \
    --arg b "$base" --arg br "$branch" \
    '. + [{number: $n, createdAt: $c, isDraft: $d, headRefOid: $h, baseRefName: $b, headRefName: $br}]' \
    "$tmp/pr_list.json" >"$tmp/pr_list.json.new"
  mv "$tmp/pr_list.json.new" "$tmp/pr_list.json"

  printf '%s' "$body" >"$tmp/pr_${n}_body.txt"
  printf '%s' "$labels" >"$tmp/pr_${n}_labels.json"
  printf '%s' "$checks" >"$tmp/pr_${n}_checks.json"
  printf '%s\n' "$diff" >"$tmp/pr_${n}_diff.txt"
  printf '{"merged": false}' >"$tmp/pr_${n}_merged.json"
  printf '%s' "$head" >"$tmp/pr_${n}_head"
  printf '%s' "$mergeable" >"$tmp/pr_${n}_mergeable"
}

run_merge() {
  prs_cut=${1:-}
  dry_run=${2:-false}
  : >"$tmp/gh.log"
  : >"$tmp/merge_requests"
  : >"$tmp/dispatch_count"
  rm -f "$tmp/dispatch_run_url"
  if ! BATCH_MERGE_PRS=$prs_cut \
    BATCH_MERGE_DRY_RUN=$dry_run \
    BATCH_MERGE_DISPATCH_POLLS=1 \
    BATCH_MERGE_DISPATCH_SLEEP=0 \
    GH_STUB_ROOT=$tmp \
    GH_REPO=Jabbslad/kiln \
    GITHUB_STEP_SUMMARY=$tmp/summary.md \
    PATH="$tmp:$PATH" \
    sh "$merge" >"$tmp/out" 2>"$tmp/err"; then
    fail "batch-merge exited non-zero: $(cat "$tmp/err") $(cat "$tmp/out")"
  fi
}

assert_skip() {
  needle=$1
  grep -q "$needle" "$tmp/out" || fail "expected skip containing: $needle (got: $(cat "$tmp/out") err=$(cat "$tmp/err"))"
}

assert_merged_order() {
  expected=$1
  actual=$(tr '\n' ' ' <"$tmp/merge_requests" | sed 's/ $//')
  [ "$actual" = "$expected" ] || fail "merge order expected '$expected' got '$actual'"
}

assert_dispatch_count() {
  count=$1
  got=$(wc -l <"$tmp/dispatch_count" | tr -d ' ')
  [ "${got:-0}" = "$count" ] || fail "expected $count dispatch(es), got ${got:-0}"
}

write_stub
printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n' >"$tmp/main_sha"

IDLE_STUB_MODE=idle GH_STUB_ROOT=$tmp GH_REPO=Jabbslad/kiln PATH="$tmp:$PATH" sh "$idle_gate" \
  || fail 'idle gate should pass when no in-progress runs'
if IDLE_STUB_MODE=busy GH_STUB_ROOT=$tmp GH_REPO=Jabbslad/kiln PATH="$tmp:$PATH" sh "$idle_gate"; then
  fail 'idle gate should refuse busy main'
fi

reset_prs
run_merge
assert_skip 'no open non-draft'
assert_dispatch_count 0

reset_prs
setup_pr 10 aaa1111111111111111111111111111111111111111 2026-01-01T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"in_progress","conclusion":null}]'
run_merge
assert_skip 'pending check runs'
assert_dispatch_count 0

reset_prs
setup_pr 11 bbb2222222222222222222222222222222222222222 2026-01-02T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"failure"}]'
run_merge
assert_skip 'not all success or skipped'
assert_dispatch_count 0

reset_prs
setup_pr 12 ccc3333333333333333333333333333333333333333 2026-01-03T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' README.md main branch-12 CONFLICTING
run_merge
assert_skip 'not mergeable'
assert_dispatch_count 0
grep -q 'batch-merge skipped' "$tmp/pr_12_comments.txt" || fail 'expected skip comment on #12'

reset_prs
setup_pr 14 eee5555555555555555555555555555555555555555 2026-01-05T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' README.md main branch-14 MERGEABLE
# behind main is allowed: no behind field in stub; merge should succeed
run_merge
grep -q 'merged pull request #14' "$tmp/out" || fail 'expected merge despite staleness'
assert_dispatch_count 1
grep -q 'batch_base_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' "$tmp/dispatch_args" \
  || fail 'dispatch should pass batch_base_sha'

reset_prs
body_dnm=$(printf 'title\nDo Not Merge\n')
setup_pr 15 fff6666666666666666666666666666666666666666 2026-01-06T00:00:00Z false "$body_dnm"
run_merge
assert_skip 'do not merge'
assert_dispatch_count 0

reset_prs
setup_pr 16 aaa7777777777777777777777777777777777777777 2026-01-07T00:00:00Z true ok
run_merge
assert_skip 'no open non-draft'

reset_prs
diff_py=$(printf 'README.md\nscripts/foo.py')
setup_pr 17 bbb8888888888888888888888888888888888888888 2026-01-08T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' "$diff_py"
run_merge
assert_skip '.py file'

reset_prs
setup_pr 20 deadbeefdeadbeefdeadbeefdeadbeefdeadbeef 2026-01-10T00:00:00Z false ok
printf '1\n' >"$tmp/pr_20_merge_fail"
run_merge
assert_skip 'merge failed'
grep -q 'batch-merge skipped' "$tmp/pr_20_comments.txt" || fail 'expected comment on merge failure'
assert_dispatch_count 0

reset_prs
setup_pr 30 1111111111111111111111111111111111111111 2026-01-01T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' README.md main stack-base
setup_pr 31 2222222222222222222222222222222222222222 2026-01-02T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' README.md stack-base stack-top
run_merge
assert_merged_order '30 31'
assert_dispatch_count 1

reset_prs
setup_pr 40 cafebabecafebabecafebabecafebabecafebabe 2026-01-11T00:00:00Z false ok
setup_pr 41 deadbeefdeadbeefdeadbeefdeadbeefdeadbeef 2026-01-12T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"failure"}]'
run_merge
assert_merged_order '40'
assert_dispatch_count 1

reset_prs
setup_pr 50 3333333333333333333333333333333333333333 2026-01-01T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' README.md main branch-50
setup_pr 51 4444444444444444444444444444444444444444 2026-01-02T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' README.md main branch-51
run_merge '51'
grep -qx 51 "$tmp/merge_requests" || fail 'cut list should merge #51'
grep -qx 50 "$tmp/merge_requests" 2>/dev/null && fail 'cut list must not merge #50'
assert_dispatch_count 1

reset_prs
setup_pr 60 5555555555555555555555555555555555555555 2026-01-01T00:00:00Z false ok
setup_pr 99 6666666666666666666666666666666666666666 2026-01-02T00:00:00Z false ok
run_merge '60 99' true
grep -q 'dry-run merge #60' "$tmp/out" || fail 'dry run should plan #60'
grep -q 'dry-run merge #99' "$tmp/out" || fail 'dry run should plan #99'
grep -q 'merge order' "$tmp/summary.md" || fail 'dry run summary should list merge order'
grep -q 'would dispatch' "$tmp/summary.md" || fail 'dry run summary should mention dispatch'
[ ! -s "$tmp/merge_requests" ] || fail 'dry run must not merge'
assert_dispatch_count 0

reset_prs
run_merge '999' true
grep -q 'not an open non-draft' "$tmp/summary.md" || fail 'missing cut-list PR should be skipped in summary'

reset_prs
setup_pr 70 7777777777777777777777777777777777777777 2026-01-13T00:00:00Z false ok
printf '%s\n' "$(printf '%040d' 0 | tr 0 b)" >"$tmp/main_after"
run_merge
assert_dispatch_count 0
moved=$(printf '%040d' 0 | tr 0 b)
grep -q "release skipped: main moved to ${moved}, the newer build releases" "$tmp/out" \
  || fail "batch-merge should skip dispatch when main moved: $(cat "$tmp/out")"
rm -f "$tmp/main_after"

printf 'batch merge tests ok\n'
