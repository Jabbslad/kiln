#!/bin/sh
# Stub gh and prove the wait. Then read the v0.4.15 runs and replay them.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
wait=$root/scripts/wait-main-run.sh
sha=910a466315f2708b02502363ab694aceb8aa471a

fail() {
  printf 'test-wait-main-run: %s\n' "$*" >&2
  exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cat >"$tmp/gh" <<'EOF'
#!/bin/sh
set -eu
log=${GH_LOG:?}
state=${GH_STATE:?}
mode=${GH_MODE:?}
run_id=${GH_RUN_ID:-8}
sha=${REUSE_SHA:?}
printf '%s\n' "$*" >>"$log"
case "$1" in
  api) ;;
  run)
    [ "${2:-}" = view ] || exit 1
    ;;
  *) exit 1 ;;
esac
n=0
if [ -f "$state" ]; then
  n=$(tr -d '[:space:]' <"$state")
fi
case $n in
  ''|*[!0-9]*) n=0 ;;
esac
n=$((n + 1))
printf '%s\n' "$n" >"$state"
if [ "$n" -eq 1 ] && [ "$mode" != ready ]; then
  printf '%s\tin_progress\t\t%s\tmain\tpush\n' "$run_id" "$sha"
  exit 0
fi
case $mode in
  success|ready) printf '%s\tcompleted\tsuccess\t%s\tmain\tpush\n' "$run_id" "$sha" ;;
  failure) printf '%s\tcompleted\tfailure\t%s\tmain\tpush\n' "$run_id" "$sha" ;;
  cancelled) printf '%s\tcompleted\tcancelled\t%s\tmain\tpush\n' "$run_id" "$sha" ;;
  timeout) printf '%s\tin_progress\t\t%s\tmain\tpush\n' "$run_id" "$sha" ;;
  *) exit 1 ;;
esac
EOF
chmod 755 "$tmp/gh"

run_wait() {
  mode=$1
  limit=$2
  run_id=$3
  : >"$tmp/state"
  : >"$tmp/log"
  : >"$tmp/out"
  : >"$tmp/err"
  set +e
  GH_MODE=$mode \
    GH_STATE=$tmp/state \
    GH_LOG=$tmp/log \
    GH_RUN_ID=$run_id \
    GITHUB_OUTPUT=$tmp/out \
    REUSE_EVENT=push \
    REUSE_SHA=$sha \
    GH_REPO=Jabbslad/kiln \
    REUSE_WAIT_LIMIT=$limit \
    REUSE_POLL_INTERVAL=1 \
    PATH="$tmp:$PATH" \
    sh "$wait" >"$tmp/stdout" 2>"$tmp/err"
  code=$?
  set -e
  if [ "$code" -ne 0 ]; then
    fail "$mode exited $code"
  fi
}

assert_waiting() {
  run_id=$1
  grep -q "reuse waiting: main run $run_id is in progress" "$tmp/err" || fail "missing waiting line for $run_id"
}

assert_refused() {
  message=$1
  grep -q "reuse refused: $message" "$tmp/err" || fail "missing refused line: $message"
  grep -q '^mode=fallback$' "$tmp/out" || fail 'fallback mode was not recorded'
  grep -q "^reason=$message$" "$tmp/out" || fail 'fallback reason was not recorded'
  grep -q '^version=$' "$tmp/out" || fail 'fallback version was not cleared'
}

assert_reused() {
  if grep -q '^mode=fallback$' "$tmp/out"; then
    fail 'a successful wait recorded a fallback'
  fi
  if grep -q 'reuse refused:' "$tmp/err"; then
    fail 'a successful wait refused'
  fi
}

run_wait success 30 8
assert_waiting 8
assert_reused
grep -q '^api ' "$tmp/log" || fail 'success case did not call gh api'
grep -q '^run view ' "$tmp/log" || fail 'success case did not call gh run view'

run_wait failure 30 8
assert_waiting 8
assert_refused 'main run 8 failed'

run_wait cancelled 30 8
assert_waiting 8
assert_refused 'main run 8 was cancelled'

started=$(date +%s)
run_wait timeout 2 8
finished=$(date +%s)
assert_waiting 8
assert_refused 'timed out waiting for main run 8'
if [ $((finished - started)) -ge 30 ]; then
  fail 'timeout override waited too long'
fi

run_wait ready 30 8
if grep -q 'reuse waiting:' "$tmp/err"; then
  fail 'a completed run should not wait'
fi
assert_reused
if grep -q '^run view ' "$tmp/log"; then
  fail 'a completed run should not be polled'
fi

workflow=$root/.github/workflows/build.yml
decide=$(awk '
  /id: decide/ { grab = 1 }
  grab { print }
  grab && /reuse-server-check.sh/ { exit }
' "$workflow")
printf '%s\n' "$decide" | grep -q 'sh scripts/wait-main-run.sh' || fail 'decide step does not wait'
printf '%s\n' "$decide" | grep -q 'mode=fallback' || fail 'decide step does not skip a fallback'
wait_line=$(printf '%s\n' "$decide" | grep -n 'wait-main-run.sh' | head -n 1 | cut -d: -f1)
fetch_line=$(printf '%s\n' "$decide" | grep -n 'cargo xtask reuse-release fetch' | head -n 1 | cut -d: -f1)
if [ -z "$wait_line" ] || [ -z "$fetch_line" ] || [ "$wait_line" -ge "$fetch_line" ]; then
  fail 'fetch runs before the wait'
fi
if ! grep -q '!cancelled()' "$workflow"; then
  fail 'workflow lost !cancelled()'
fi

command -v curl >/dev/null 2>&1 || fail 'curl is not installed'
command -v jq >/dev/null 2>&1 || fail 'jq is not installed'
read_run() {
  attempt=0
  while [ "$attempt" -lt 4 ]; do
    attempt=$((attempt + 1))
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      if curl -fsS \
        -H 'Accept: application/vnd.github+json' \
        -H 'User-Agent: kiln-wait-test' \
        -H "Authorization: Bearer ${GITHUB_TOKEN}" \
        "$1"; then
        return 0
      fi
    elif curl -fsS \
      -H 'Accept: application/vnd.github+json' \
      -H 'User-Agent: kiln-wait-test' \
      "$1"; then
      return 0
    fi
    if [ "$attempt" -ge 4 ]; then
      return 1
    fi
    sleep $((attempt * 2))
  done
}

tag=$(read_run https://api.github.com/repos/Jabbslad/kiln/actions/runs/39523146863)
main=$(read_run https://api.github.com/repos/Jabbslad/kiln/actions/runs/39523085432)
tag_sha=$(printf '%s\n' "$tag" | jq -r .head_sha)
main_id=$(printf '%s\n' "$main" | jq -r .id)
main_sha=$(printf '%s\n' "$main" | jq -r .head_sha)
main_branch=$(printf '%s\n' "$main" | jq -r .head_branch)
main_event=$(printf '%s\n' "$main" | jq -r .event)
main_path=$(printf '%s\n' "$main" | jq -r .path)
main_status=$(printf '%s\n' "$main" | jq -r .status)
main_conclusion=$(printf '%s\n' "$main" | jq -r .conclusion)
main_started=$(printf '%s\n' "$main" | jq -r .run_started_at)
main_finished=$(printf '%s\n' "$main" | jq -r .updated_at)
[ "$tag_sha" = "$sha" ] || fail "tag run is $tag_sha"
[ "$main_id" = 39523085432 ] || fail "main run is $main_id"
[ "$main_sha" = "$sha" ] || fail 'main run is a different commit'
[ "$main_branch" = main ] || fail 'main run is not on main'
[ "$main_event" = push ] || fail 'main run is not a push'
[ "$main_path" = .github/workflows/build.yml ] || fail 'main run is not build.yml'
[ "$main_status" = completed ] || fail 'main run did not complete'
[ "$main_conclusion" = success ] || fail 'main run did not succeed'

tag_jobs=$(read_run 'https://api.github.com/repos/Jabbslad/kiln/actions/runs/39523146863/jobs?per_page=100')
main_jobs=$(read_run 'https://api.github.com/repos/Jabbslad/kiln/actions/runs/39523085432/jobs?per_page=100')
decided=$(printf '%s\n' "$tag_jobs" | jq -r '.jobs[] | select(.name == "reuse") | .completed_at')
server=$(printf '%s\n' "$main_jobs" | jq -r '.jobs[] | select(.name == "server-check") | .conclusion')
[ "$server" = success ] || fail 'server-check did not succeed'
[ -n "$decided" ] || fail 'tag reuse job has no completion time'
if [ "$main_started" = "$decided" ] || [ "$decided" = "$main_finished" ]; then
  fail 'timestamps collapsed'
fi
order=$(printf '%s\n%s\n%s\n' "$main_started" "$decided" "$main_finished" | sort)
expect=$(printf '%s\n%s\n%s\n' "$main_started" "$decided" "$main_finished")
[ "$order" = "$expect" ] || fail 'main was not in progress when the tag reuse job finished'

run_wait success 30 39523085432
assert_waiting 39523085432
assert_reused
grep -q '^run view 39523085432 ' "$tmp/log" || fail 'replay did not poll the real main run'
