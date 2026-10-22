#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
merge=$root/scripts/slow-lane-merge.sh

fail() {
  printf 'test-slow-lane-merge: %s\n' "$*" >&2
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
  label)
    shift
    case "$1" in
      list)
        shift
        while [ $# -gt 0 ]; do
          case "$1" in
            --jq) jq_filter=$2; shift 2 ;;
            *) shift ;;
          esac
        done
        if [ -f "$root/label_exists" ]; then
          data='[{"name":"slow-lane"}]'
        else
          data='[]'
        fi
        printf '%s\n' "$data" | jq -r "$jq_filter"
        ;;
      create)
        printf '1\n' >"$root/label_created"
        ;;
      *) exit 1 ;;
    esac
    ;;
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
        printf 'true\n' >"$root/pr_${n}_merged_after"
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
      */compare/main...*)
        head=${path#*/compare/main...}
        n=$(pr_for_head "$head" || true)
        if [ -n "$n" ] && [ -f "$root/pr_${n}_behind" ]; then
          payload=$(jq -n --argjson b "$(cat "$root/pr_${n}_behind")" '{behind_by: $b}')
        else
          payload=$(jq -n '{behind_by: 0}')
        fi
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
    printf '1\n' >>"$root/dispatch_count"
    ;;
  *) exit 1 ;;
esac
STUB
  chmod 755 "$tmp/gh"
}

reset_prs() {
  printf '[]' >"$tmp/pr_list.json"
  rm -f "$tmp"/pr_*_body.txt "$tmp"/pr_*_labels.json "$tmp"/pr_*_checks.json \
    "$tmp"/pr_*_behind "$tmp"/pr_*_diff.txt "$tmp"/pr_*_merged.json \
    "$tmp"/pr_*_head "$tmp"/pr_*_merged_after
}

setup_pr() {
  n=$1
  head=$2
  created=$3
  draft=${4:-false}
  body=${5:-ok}
  labels=${6:-'{"labels":[]}'}
  checks=${7:-'[{"status":"completed","conclusion":"success"}]'}
  behind=${8:-0}
  diff=${9:-README.md}

  if [ ! -f "$tmp/pr_list.json" ]; then
    printf '[]' >"$tmp/pr_list.json"
  fi
  jq --argjson n "$n" --arg c "$created" --argjson d "$draft" --arg h "$head" \
    '. + [{number: $n, createdAt: $c, isDraft: $d, headRefOid: $h}]' "$tmp/pr_list.json" >"$tmp/pr_list.json.new"
  mv "$tmp/pr_list.json.new" "$tmp/pr_list.json"

  printf '%s' "$body" >"$tmp/pr_${n}_body.txt"
  printf '%s' "$labels" >"$tmp/pr_${n}_labels.json"
  printf '%s' "$checks" >"$tmp/pr_${n}_checks.json"
  printf '%s' "$behind" >"$tmp/pr_${n}_behind"
  printf '%s\n' "$diff" >"$tmp/pr_${n}_diff.txt"
  printf '{"merged": false}' >"$tmp/pr_${n}_merged.json"
  printf '%s' "$head" >"$tmp/pr_${n}_head"
}

run_merge() {
  : >"$tmp/gh.log"
  : >"$tmp/merge_requests"
  : >"$tmp/dispatch_count"
  GH_STUB_ROOT=$tmp \
    GH_REPO=Jabbslad/kiln \
    GITHUB_STEP_SUMMARY=$tmp/summary.md \
    PATH="$tmp:$PATH" \
    sh "$merge" >"$tmp/out" 2>"$tmp/err"
}

assert_skip() {
  needle=$1
  grep -q "$needle" "$tmp/out" || fail "expected skip containing: $needle (got: $(cat "$tmp/out") err=$(cat "$tmp/err"))"
  grep -q "$needle" "$tmp/summary.md" || fail "summary missing: $needle"
}

assert_merged() {
  n=$1
  head=$2
  grep -qx "$n" "$tmp/merge_requests" || fail "expected merge of #$n"
  grep -qx "$head" "$tmp/merge_heads" || fail "expected head $head on merge"
  [ "$(wc -l <"$tmp/merge_requests" | tr -d ' ')" = 1 ] || fail 'expected exactly one merge'
  [ "$(wc -l <"$tmp/dispatch_count" | tr -d ' ')" = 1 ] || fail 'expected exactly one workflow dispatch'
}

write_stub
printf '1\n' >"$tmp/label_exists"

reset_prs
run_merge
assert_skip 'no open non-draft'

reset_prs
setup_pr 10 aaa1111111111111111111111111111111111111111 2026-01-01T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"in_progress","conclusion":null}]' 0 README.md
run_merge
assert_skip 'pending check runs'

reset_prs
setup_pr 11 bbb2222222222222222222222222222222222222222 2026-01-02T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"failure"}]' 0 README.md
run_merge
assert_skip 'not all success or skipped'

reset_prs
setup_pr 12 ccc3333333333333333333333333333333333333333 2026-01-03T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' 2 README.md
run_merge
assert_skip 'behind main'

reset_prs
setup_pr 13 ddd4444444444444444444444444444444444444444 2026-01-04T00:00:00Z false $'title\nDo Not Merge\n' \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' 0 README.md
run_merge
assert_skip 'do not merge'

reset_prs
setup_pr 14 eee5555555555555555555555555555555555555555 2026-01-05T00:00:00Z false ok \
  '{"labels":[{"name":"hold"}]}' '[{"status":"completed","conclusion":"success"}]' 0 README.md
run_merge
assert_skip 'hold or gated'

reset_prs
setup_pr 15 fff6666666666666666666666666666666666666666 2026-01-06T00:00:00Z false ok \
  '{"labels":[{"name":"gated"}]}' '[{"status":"completed","conclusion":"success"}]' 0 README.md
run_merge
assert_skip 'hold or gated'

reset_prs
setup_pr 16 aaa7777777777777777777777777777777777777777 2026-01-07T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"}]' 0 $'README.md\nscripts/foo.py'
run_merge
assert_skip '.py file'

reset_prs
setup_pr 20 deadbeefdeadbeefdeadbeefdeadbeefdeadbeef 2026-01-10T00:00:00Z false ok \
  '{"labels":[]}' '[{"status":"completed","conclusion":"success"},{"status":"completed","conclusion":"skipped"}]' 0 README.md
run_merge
assert_merged 20 deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
grep -q 'merged.*#20' "$tmp/summary.md" || fail 'summary should record merge'

reset_prs
jq -n \
  '[{number: 30, createdAt: "2026-01-01T00:00:00Z", isDraft: false, headRefOid: "1111111111111111111111111111111111111111"},
    {number: 31, createdAt: "2026-01-02T00:00:00Z", isDraft: false, headRefOid: "2222222222222222222222222222222222222222"}]' \
  >"$tmp/pr_list.json"
printf 'ok' >"$tmp/pr_30_body.txt"
printf 'ok' >"$tmp/pr_31_body.txt"
printf '{"labels":[]}' >"$tmp/pr_30_labels.json"
printf '{"labels":[]}' >"$tmp/pr_31_labels.json"
printf '[{"status":"completed","conclusion":"success"}]\n' >"$tmp/pr_30_checks.json"
printf '[{"status":"completed","conclusion":"success"}]\n' >"$tmp/pr_31_checks.json"
printf '0\n' >"$tmp/pr_30_behind"
printf '0\n' >"$tmp/pr_31_behind"
printf 'README.md\n' >"$tmp/pr_30_diff.txt"
printf 'README.md\n' >"$tmp/pr_31_diff.txt"
printf '{"merged": false}' >"$tmp/pr_30_merged.json"
printf '{"merged": false}' >"$tmp/pr_31_merged.json"
printf '1111111111111111111111111111111111111111' >"$tmp/pr_30_head"
printf '2222222222222222222222222222222222222222' >"$tmp/pr_31_head"
run_merge
assert_merged 30 1111111111111111111111111111111111111111
grep -qx 31 "$tmp/merge_requests" 2>/dev/null && fail 'second PR must not merge in the same run'

reset_prs
printf '[{"number":40,"createdAt":"2026-01-01T00:00:00Z","isDraft":true,"headRefOid":"abc"}]' >"$tmp/pr_list.json"
run_merge
assert_skip 'no open non-draft'

rm -f "$tmp/label_exists"
reset_prs
run_merge
[ -f "$tmp/label_created" ] || fail 'expected label create when missing'
assert_skip 'no open non-draft'

printf 'slow-lane merge tests ok\n'
