#!/bin/sh
# The 4 GiB nextest cap fails only when nextest exits non-zero or oom_kill > 0.
# Peak at the cap and memory.events max are reclaim, not a kill.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
script=$root/scripts/test-nextest-memory-cap.sh

fail() {
  printf 'test-nextest-cap-verdict: %s\n' "$*" >&2
  exit 1
}

[ -f "$script" ] || fail 'missing test-nextest-memory-cap.sh'
grep -q '^cap=4294967296$' "$script" \
  || fail 'memory.max must stay 4 GiB (4294967296)'
grep -F -q "printf '%s\\n' \"\$cap\" > \"\$cg/memory.max\"" "$script" \
  || fail 'the real run must still write memory.max from the 4 GiB cap'
grep -q '\[ "${1:-}" = --verdict \]' "$script" \
  || fail 'missing --verdict; hermetic cgroup files cannot call the rule'
grep -q 'peak_bytes=%s max_events=%s oom_kill=%s' "$script" \
  || fail 'the verdict must keep printing peak and max events'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

write_cg() {
  dir=$1
  peak=$2
  max_events=$3
  oom_kill=$4
  mkdir -p "$dir"
  printf '%s\n' "$peak" >"$dir/memory.peak"
  cat >"$dir/memory.events" <<EOF
low 0
high 0
max $max_events
oom 0
oom_kill $oom_kill
oom_group_kill 0
EOF
}

expect_ok() {
  name=$1
  dir=$2
  code=$3
  err=$work/$name.err
  if ! sh "$script" --verdict "$code" "$dir" >"$work/$name.out" 2>"$err"; then
    cat "$err" >&2
    fail "$name should pass"
  fi
  grep -q 'peak_bytes=' "$err" || fail "$name did not print peak"
  grep -q 'max_events=' "$err" || fail "$name did not print max events"
}

expect_fail() {
  name=$1
  dir=$2
  code=$3
  if sh "$script" --verdict "$code" "$dir" >"$work/$name.out" 2>"$work/$name.err"; then
    fail "$name should fail"
  fi
}

# Peak equal to the cap, reclaim events, nothing killed.
write_cg "$work/ceiling" 4294967296 425 0
expect_ok ceiling "$work/ceiling" 0
grep -q 'peak_bytes=4294967296' "$work/ceiling.err" \
  || fail 'ceiling case did not print peak_bytes=4294967296'
grep -q 'max_events=425' "$work/ceiling.err" \
  || fail 'ceiling case did not print max_events=425'
grep -q 'oom_kill=0' "$work/ceiling.err" \
  || fail 'ceiling case did not print oom_kill=0'

# A real kill fails even when nextest's exit code was lost.
write_cg "$work/killed" 4294967296 1 1
expect_fail killed "$work/killed" 0

# Nextest itself failing is a failure even with oom_kill=0 and room to spare.
write_cg "$work/nonzero" 1048576 0 0
expect_fail nonzero "$work/nonzero" 7

printf 'nextest cap verdict ok\n'
