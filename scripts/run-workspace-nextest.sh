#!/bin/sh
# Run the workspace test suite with cargo-nextest.
# Shards run in parallel when the machine has RAM to spare. On a low-memory
# host (about 5 GiB, or a cgroup cap) they run one at a time with debuginfo
# off, so three linkers are not resident together.
#
# scripts/flaky-tests.txt lists known-flaky tests. Each gets one nextest retry
# on the selected profile. Every other test gets none. Retries are counted in
# target/flaky-retries.log. The repo .config/nextest.toml (slow-timeout and
# the rest) is kept.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# A global retry count disables per-test overrides. The flaky list is the policy.
unset NEXTEST_RETRIES || true

shards=${NEXTEST_SHARDS:-3}
case "$shards" in
  '' | *[!0-9]* | 0) echo "run-workspace-nextest: NEXTEST_SHARDS must be a positive integer" >&2; exit 1 ;;
esac

profile=${NEXTEST_PROFILE:-ci}

serial=0
if [ "${KILN_NEXTEST_SERIAL:-}" = 1 ]; then
  serial=1
elif [ "${KILN_NEXTEST_SERIAL:-}" != 0 ] && sh "$root/scripts/nextest-low-memory.sh"; then
  serial=1
fi

if [ "$serial" -eq 1 ]; then
  : "${CARGO_PROFILE_DEV_DEBUG:=0}"
  : "${CARGO_PROFILE_TEST_DEBUG:=0}"
  export CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cfg=$tmp/nextest.toml
list=$root/scripts/flaky-tests.txt
repo_cfg=$root/.config/nextest.toml
header="[profile.${profile}]"

if [ -f "$repo_cfg" ]; then
  cp "$repo_cfg" "$cfg"
else
  : >"$cfg"
fi
if ! grep -F -x -q -- "$header" "$cfg"; then
  printf '\n%s\n' "$header" >>"$cfg"
fi
if ! grep -qx 'retries = 0' "$cfg"; then
  awk -v header="$header" '
    $0 == header && !inserted { print; print "retries = 0"; inserted = 1; next }
    { print }
  ' "$cfg" >"$cfg.new"
  mv "$cfg.new" "$cfg"
fi
if [ -f "$list" ]; then
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      '' | \#*) continue ;;
    esac
    pkg=${line%% *}
    rest=${line#"$pkg"}
    rest=${rest#" "}
    name=${rest%% *}
    if [ -z "$pkg" ] || [ -z "$name" ] || [ "$name" = "$pkg" ]; then
      printf 'run-workspace-nextest: bad flaky list line: %s\n' "$line" >&2
      exit 1
    fi
    {
      printf '[[profile.%s.overrides]]\n' "$profile"
      printf "filter = 'package(%s) + test(%s)'\n" "$pkg" "$name"
      printf '%s\n' 'retries = 1' ''
    } >>"$cfg"
  done <"$list"
fi

extract_retries() {
  src=$1
  dest=$2
  : >"$dest"
  esc=$(printf '\033')
  sed "s/${esc}\\[[0-9;]*m//g" "$src" >"$dest.plain"
  while IFS= read -r line; do
    case "$line" in
      *RETRY\ [1-9]*)
        printf '%s\n' "$line" >>"$dest"
        ;;
    esac
  done <"$dest.plain"
}

run_logged() {
  log=$1
  retry=$2
  shift 2
  set +e
  cargo nextest run --profile "$profile" --config-file "$cfg" "$@" >"$log" 2>&1
  code=$?
  set -e
  cat "$log"
  extract_retries "$log" "$retry"
  return "$code"
}

run_partition() {
  shard=$1
  run_logged "$tmp/shard-$shard.out" "$tmp/shard-$shard.retry" \
    --workspace --all-features --locked --exclude xtask --exclude kiln-server \
    --partition "count:$shard/${shards}"
}

fail=0
if [ "$serial" -eq 1 ]; then
  shard=1
  while [ "$shard" -le "$shards" ]; do
    if ! run_partition "$shard"; then
      fail=1
    fi
    shard=$((shard + 1))
  done
else
  pids=
  for shard in $(seq 1 "$shards"); do
    run_partition "$shard" &
    pids="$pids $!"
  done
  for pid in $pids; do
    if ! wait "$pid"; then
      fail=1
    fi
  done
fi

if ! run_logged "$tmp/server.out" "$tmp/server.retry" \
  -p kiln-server --all-features --locked --test-threads=1; then
  fail=1
fi

combined=$tmp/retries-all
: >"$combined"
cat "$tmp"/shard-*.retry "$tmp/server.retry" >>"$combined"
mkdir -p "$root/target"
retry_log=$root/target/flaky-retries.log
: >"$retry_log"
total=0
if [ -f "$list" ]; then
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      '' | \#*) continue ;;
    esac
    pkg=${line%% *}
    rest=${line#"$pkg"}
    rest=${rest#" "}
    name=${rest%% *}
    [ -n "$name" ] || continue
    count=$(grep -F -c -e "$name" "$combined" || true)
    if [ "$count" -gt 0 ]; then
      printf 'flaky-retry: %s %s count=%s\n' "$pkg" "$name" "$count" >>"$retry_log"
      total=$((total + count))
    fi
  done <"$list"
fi
printf 'flaky-retry: total=%s\n' "$total" >>"$retry_log"
cat "$retry_log"

exit "$fail"
