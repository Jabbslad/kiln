#!/bin/sh
# Run the workspace test suite with cargo-nextest.
# Shards run in parallel when the machine has RAM to spare. On a low-memory
# host (about 5 GiB, or a cgroup cap) they run one at a time with debuginfo
# off, so three linkers are not resident together.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

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

run_partition() {
  cargo nextest run --profile "$profile" --workspace --all-features --locked --exclude xtask --exclude kiln-server \
    --partition "count:$1/${shards}"
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

if ! cargo nextest run --profile "$profile" -p kiln-server --all-features --locked --test-threads=1; then
  fail=1
fi
exit "$fail"
