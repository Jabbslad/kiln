#!/bin/bash
# Host-side dashboard metrics check for smoke (fetch + POSIX awk). Deploy uses
# fetch-dashboard-metrics.sh on the host and verify-dashboard-host-metrics.sh on the runner.
set -euo pipefail
umask 077

fail() { printf 'dashboard-metrics: %s\n' "$*" >&2; exit 1; }

address=${1:?usage: check-dashboard-metrics.sh ADDRESS}
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

host_metric() {
  local body=$1 field=$2
  printf '%s' "$body" | tr -d '\n' | awk -v field="$field" '
  {
    host = index($0, "\"host\":")
    if (host == 0) exit 1
    rest = substr($0, host)
    metrics = index(rest, "\"metrics\":")
    if (metrics == 0) exit 1
    rest = substr(rest, metrics)
    key = "\"" field "\":"
    at = index(rest, key)
    if (at == 0) exit 1
    rest = substr(rest, at + length(key))
    sub(/^[[:space:]]*/, "", rest)
    if (rest !~ /^[0-9]/) exit 1
    match(rest, /^[0-9]+(\.[0-9]+)?/)
    print substr(rest, RSTART, RLENGTH)
  }'
}

body=$(bash "$script_dir/fetch-dashboard-metrics.sh" "$address")
host_cpu=$(host_metric "$body" cpu_percent)
[ -n "$host_cpu" ] || fail 'host cpu_percent is missing or null'
host_mem=$(host_metric "$body" memory_bytes)
[ -n "$host_mem" ] || fail 'host memory_bytes is missing or null'
[ "$host_mem" -gt 0 ] || fail 'host memory_bytes must be positive'

running=$(printf '%s' "$body" | tr -d '\n' | awk '
  {
    if ($0 !~ /"state":"running"/) {
      print "0"
      exit 0
    }
    print "1"
  }
')
if [ "$running" = "0" ]; then
  printf 'dashboard-metrics host cpu=%s memory=%s (no running box)\n' "$host_cpu" "$host_mem"
  exit 0
fi

body=$(bash "$script_dir/fetch-dashboard-metrics.sh" "$address")
box_mem=$(printf '%s' "$body" | tr -d '\n' | awk '
  {
    while (match($0, /"state":"running"/)) {
      rest = substr($0, RSTART)
      cpu_key = "\"cpu_percent\":"
      mem_key = "\"memory_bytes\":"
      cpu_at = index(rest, cpu_key)
      mem_at = index(rest, mem_key)
      if (cpu_at == 0 || mem_at == 0) exit 1
      cpu_rest = substr(rest, cpu_at + length(cpu_key))
      sub(/^[[:space:]]*/, "", cpu_rest)
      if (cpu_rest !~ /^[0-9]/) exit 1
      mem_rest = substr(rest, mem_at + length(mem_key))
      sub(/^[[:space:]]*/, "", mem_rest)
      if (mem_rest !~ /^[0-9]/) exit 1
      print "ok"
      exit 0
    }
    exit 1
  }
') || fail 'running box metrics are missing or null'
printf 'dashboard-metrics host cpu=%s memory=%s %s\n' "$host_cpu" "$host_mem" "$box_mem"
