#!/bin/sh
# Guard: server-check uses cargo-nextest with parallel partitions on one runner.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow=$root/.github/workflows/build.yml
check=$root/scripts/check.sh
runner=$root/scripts/run-workspace-nextest.sh

fail() {
  printf 'test-server-check-nextest: %s\n' "$*" >&2
  exit 1
}

[ -x "$runner" ] || fail 'missing run-workspace-nextest.sh'

block=$(
  awk '/^  server-check:/{flag=1} flag && /^  [a-z].*:/ && !/^  server-check:/{exit} flag{print}' \
    "$workflow"
)
printf '%s\n' "$block" | grep -q 'tool: cargo-nextest' \
  || fail 'server-check must install cargo-nextest'
printf '%s\n' "$block" | grep -q 'run-workspace-nextest.sh' \
  || fail 'server-check must run workspace tests through run-workspace-nextest.sh'
printf '%s\n' "$block" | grep -q 'Swatinem/rust-cache@' \
  || fail 'server-check must keep Swatinem/rust-cache'

grep -q 'run-workspace-nextest.sh' "$check" \
  || fail 'check.sh must use run-workspace-nextest.sh'
grep -q 'NEXTEST_SHARDS' "$runner" \
  || fail 'run-workspace-nextest.sh must shard with NEXTEST_SHARDS'
grep -q 'nextest-low-memory.sh' "$runner" \
  || fail 'run-workspace-nextest.sh must consult nextest-low-memory.sh'
grep -q 'CARGO_PROFILE_TEST_DEBUG' "$runner" \
  || fail 'low-memory nextest must drop test debuginfo'
grep -q 'test-nextest-memory-cap.sh' "$check" \
  || fail 'check.sh must run the memory-cap check outside CI'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cat > "$work/cargo" <<'EOF'
#!/bin/sh
dir=${NEXTEST_FAKE_DIR:?}
name=$$
printf '%s\n' "$name" > "$dir/$name"
printf '%s\n' "${CARGO_PROFILE_TEST_DEBUG-unset}" >> "$dir/debug"
sleep 0.4
overlap=0
for pidfile in "$dir"/[0-9]*; do
  [ -e "$pidfile" ] || continue
  pid=${pidfile##*/}
  if [ "$pid" != "$name" ] && kill -0 "$pid" 2>/dev/null; then
    overlap=1
  fi
done
if [ "$overlap" -eq 1 ]; then
  printf '1\n' >> "$dir/overlap"
fi
rm -f "$dir/$name"
exit 0
EOF
chmod +x "$work/cargo"

run_fake() {
  mode=$1
  dir=$work/mode-$mode
  mkdir -p "$dir"
  unset CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG
  NEXTEST_FAKE_DIR=$dir \
    KILN_NEXTEST_SERIAL=$mode \
    PATH="$work:$PATH" \
    sh "$runner"
  if [ "$mode" = 1 ]; then
    [ ! -s "$dir/overlap" ] || fail 'serial mode overlapped nextest shards'
    grep -qx 0 "$dir/debug" || fail 'serial mode must set test debuginfo to 0'
  else
    [ -s "$dir/overlap" ] || fail 'parallel mode did not overlap nextest shards'
    grep -qx unset "$dir/debug" || fail 'parallel mode must leave test debuginfo unset'
  fi
}

run_fake 1
run_fake 0

printf 'server-check nextest guard ok\n'
