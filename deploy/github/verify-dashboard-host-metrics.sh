#!/bin/bash
# Validate host.metrics on a dashboard JSON body. Runs on the GitHub runner (jq).
set -euo pipefail

fail() { printf 'dashboard-metrics: %s\n' "$*" >&2; exit 1; }

command -v jq >/dev/null 2>&1 || fail 'jq is required'
body=$(cat)
[ -n "$body" ] || fail 'dashboard body is empty'

host_cpu=$(printf '%s' "$body" | jq -er '.host.metrics.cpu_percent | select(type == "number")') \
  || fail 'host cpu_percent is missing or null'
host_mem=$(printf '%s' "$body" | jq -er '.host.metrics.memory_bytes | select(type == "number")') \
  || fail 'host memory_bytes is missing or null'
[ "$host_mem" -gt 0 ] || fail 'host memory_bytes must be positive'

printf 'dashboard-metrics host cpu=%s memory=%s\n' "$host_cpu" "$host_mem"
