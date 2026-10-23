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

# A virtio balloon still holding 2 GiB or more is RAM the host can take back.
# Count it when it is tighter than MemAvailable, so a half-deflated agent VM
# does not look like a 16 GiB CI runner.
if [ -r /proc/vmstat ]; then
  inf=$(awk '/^balloon_inflate / { print $2; exit }' /proc/vmstat)
  def=$(awk '/^balloon_deflate / { print $2; exit }' /proc/vmstat)
  inf=${inf:-0}
  def=${def:-0}
  case $inf$def in
    *[!0-9]*) inf=0; def=0 ;;
  esac
  if [ "$inf" -ge "$def" ]; then
    held=$(((inf - def) * 4096))
    if [ "$held" -ge 2147483648 ] && [ "$held" -lt "$budget" ]; then
      budget=$held
    fi
  fi
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
