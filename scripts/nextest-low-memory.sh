#!/bin/sh
# Exit 0 when workspace nextest must not run shards in parallel.
# Agent VMs have about 5 GiB available and a cgroup memory.high near 4 GiB.
# GitHub-hosted runners have 16 GiB, so CI stays on the parallel path.
set -eu

# 8 GiB. Above this VM and below a 16 GiB runner.
threshold=${KILN_NEXTEST_SERIAL_BELOW_BYTES:-8589934592}

budget=$(awk '/^MemAvailable:/ { print $2 * 1024; exit }' /proc/meminfo)
if [ -z "$budget" ]; then
  budget=$threshold
fi

self=$(awk -F: '$1 == "0" { print $3; exit }' /proc/self/cgroup)
if [ -n "$self" ]; then
  for name in memory.max memory.high; do
    file=/sys/fs/cgroup$self/$name
    if [ ! -r "$file" ]; then
      continue
    fi
    val=$(cat "$file" 2>/dev/null || true)
    case $val in
      '' | max | *[!0-9]*) continue ;;
    esac
    if [ "$val" -lt "$budget" ]; then
      budget=$val
    fi
  done
fi

if [ "$budget" -lt "$threshold" ]; then
  exit 0
fi
exit 1
