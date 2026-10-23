#!/bin/sh
# Run workspace nextest under a 4 GiB memory.max. Fail only when nextest exits
# non-zero or the cgroup records oom_kill > 0. Peak at the cap and memory.events
# max are reclaim while the hard cap holds; they are not a kill. 4 GiB stays
# below this VM's memory.high so a real overrun dies in the cgroup.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# 4 GiB.
cap=4294967296

fail() {
  printf 'test-nextest-memory-cap: %s\n' "$*" >&2
  exit 1
}

# code is nextest's exit status. cg is a cgroup directory with memory.peak and
# memory.events. --verdict uses the same rule as the real run.
verdict() {
  code=$1
  cg=$2
  peak=$(cat "$cg/memory.peak" 2>/dev/null || echo 0)
  max_events=$(awk '/^max / { print $2; exit }' "$cg/memory.events" 2>/dev/null || echo 0)
  oom_kill=$(awk '/^oom_kill / { print $2; exit }' "$cg/memory.events" 2>/dev/null || echo 0)
  peak=${peak:-0}
  max_events=${max_events:-0}
  oom_kill=${oom_kill:-0}
  printf 'test-nextest-memory-cap: exit=%s peak_bytes=%s max_events=%s oom_kill=%s\n' \
    "$code" "$peak" "$max_events" "$oom_kill" >&2
  if [ "$code" -ne 0 ]; then
    fail "nextest exited $code under the 4 GiB cap (peak_bytes=$peak oom_kill=$oom_kill)"
  fi
  if [ "$oom_kill" -ne 0 ]; then
    fail "nextest oom_kill=$oom_kill under the 4 GiB cap (peak_bytes=$peak max_events=$max_events)"
  fi
  printf 'nextest memory cap ok peak_bytes=%s\n' "$peak"
}

if [ "${1:-}" = --verdict ]; then
  code=${2:?}
  cg=${3:?}
  verdict "$code" "$cg"
  exit 0
fi

self=$(awk -F: '$1 == "0" { print $3; exit }' /proc/self/cgroup)
if [ -z "$self" ] || [ ! -d "/sys/fs/cgroup$self" ]; then
  fail 'cgroup v2 is required'
fi
parent=$(dirname "/sys/fs/cgroup$self")
if ! grep -qw memory "$parent/cgroup.subtree_control" 2>/dev/null; then
  fail 'parent cgroup does not delegate the memory controller'
fi

cg=$parent/kiln-nextest-cap-$$
mkdir "$cg"
cleanup() {
  if [ -d "$cg" ]; then
    if [ -s "$cg/cgroup.procs" ]; then
      printf '1\n' > "$cg/cgroup.kill" 2>/dev/null || true
      sleep 0.3
    fi
    rmdir "$cg" 2>/dev/null || true
  fi
}
trap cleanup EXIT

printf '1\n' > "$cg/memory.oom.group" 2>/dev/null || true
printf '%s\n' "$cap" > "$cg/memory.max"
if [ -w "$cg/memory.swap.max" ]; then
  printf '0\n' > "$cg/memory.swap.max" || true
fi

set +e
sh -c 'printf "%s\n" "$$" > "$1" || exit 99; shift; exec "$@"' mover \
  "$cg/cgroup.procs" \
  sh "$root/scripts/run-workspace-nextest.sh"
code=$?
set -e

verdict "$code" "$cg"
