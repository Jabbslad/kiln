#!/bin/bash
# Host-side dashboard metrics check for smoke (fetch + sqlite3 JSON). Deploy uses
# fetch-dashboard-metrics.sh on the host and verify-dashboard-host-metrics.sh on the runner.
set -euo pipefail
umask 077

fail() { printf 'dashboard-metrics: %s\n' "$*" >&2; exit 1; }

command -v sqlite3 >/dev/null 2>&1 || fail 'sqlite3 is required'

address=${1:?usage: check-dashboard-metrics.sh ADDRESS}
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

json_doc_file=

json_doc_cleanup() {
  if [ -n "${json_doc_file:-}" ]; then
    rm -f "$json_doc_file"
    json_doc_file=
  fi
}

load_doc() {
  json_doc_cleanup
  json_doc_file=$(mktemp)
  bash "$script_dir/fetch-dashboard-metrics.sh" "$address" >"$json_doc_file"
}

json_host_scalar() {
  local field=$1
  sqlite3 -batch :memory: "
SELECT json_extract(json(readfile('${json_doc_file}')), '$.host.metrics.${field}')
WHERE json_type(json(readfile('${json_doc_file}')), '$.host.metrics.${field}')
  IN ('integer', 'real');
"
}

has_running_box() {
  sqlite3 -batch :memory: "
SELECT 1
FROM json_each(json(readfile('${json_doc_file}')), '\$.boxes') AS b
WHERE json_extract(b.value, '\$.state') = 'running'
LIMIT 1;
"
}

running_box_metrics_ok() {
  sqlite3 -batch :memory: "
SELECT 'ok'
FROM json_each(json(readfile('${json_doc_file}')), '\$.boxes') AS b
WHERE json_extract(b.value, '\$.state') = 'running'
  AND json_type(b.value, '\$.metrics.cpu_percent') IN ('integer', 'real')
  AND json_type(b.value, '\$.metrics.memory_bytes') IN ('integer', 'real')
LIMIT 1;
"
}

trap json_doc_cleanup EXIT

load_doc
host_cpu=$(json_host_scalar cpu_percent)
[ -n "$host_cpu" ] || fail 'host cpu_percent is missing or null'
host_mem=$(json_host_scalar memory_bytes)
[ -n "$host_mem" ] || fail 'host memory_bytes is missing or null'
mem_ok=$(sqlite3 -batch :memory: "
SELECT 1
WHERE json_extract(json(readfile('${json_doc_file}')), '\$.host.metrics.memory_bytes') > 0;
")
[ -n "$mem_ok" ] || fail 'host memory_bytes must be positive'

if [ -z "$(has_running_box)" ]; then
  printf 'dashboard-metrics host cpu=%s memory=%s (no running box)\n' "$host_cpu" "$host_mem"
  exit 0
fi

load_doc
box_ok=$(running_box_metrics_ok)
[ -n "$box_ok" ] || fail 'running box metrics are missing or null'
printf 'dashboard-metrics host cpu=%s memory=%s %s\n' "$host_cpu" "$host_mem" "$box_ok"
