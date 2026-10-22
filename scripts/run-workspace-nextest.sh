#!/bin/sh
# Run the workspace test suite with cargo-nextest, sharded in parallel on this runner.
set -eu

shards=${NEXTEST_SHARDS:-3}
case "$shards" in
  '' | *[!0-9]* | 0) echo "run-workspace-nextest: NEXTEST_SHARDS must be a positive integer" >&2; exit 1 ;;
esac

pids=
for shard in $(seq 1 "$shards"); do
  cargo nextest run --workspace --all-features --locked --exclude xtask \
    --partition "count:${shard}/${shards}" &
  pids="$pids $!"
done

fail=0
for pid in $pids; do
  if ! wait "$pid"; then
    fail=1
  fi
done
exit "$fail"
