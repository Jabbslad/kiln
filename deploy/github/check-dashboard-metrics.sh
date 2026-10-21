#!/bin/bash
# Require live host metrics and non-null metrics on any running box.
set -euo pipefail
umask 077

fail() { printf 'dashboard-metrics: %s\n' "$*" >&2; exit 1; }

address=${1:?usage: check-dashboard-metrics.sh ADDRESS}
printf '%s\n' "$address" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$' \
  || fail 'address must be an IPv4 literal'
token_file=${KILN_ADMIN_TOKEN_FILE:-/etc/kiln/admin.token}
ca_file=${KILN_CA_FILE:-/etc/kiln/ca.crt}
[ -r "$token_file" ] || fail "missing admin token: ${token_file}"
[ -r "$ca_file" ] || fail "missing CA file: ${ca_file}"
token=$(tr -d '[:space:]' <"$token_file")
[ "${#token}" -eq 64 ] || fail 'admin token must be 64 hex characters'

url="https://${address}:8443/api/v1/dashboard"
fetch() {
  curl --fail --silent --show-error \
    --cacert "$ca_file" \
    -H "Authorization: Bearer ${token}" \
    "$url"
}

body=$(fetch)
host_cpu=$(printf '%s' "$body" | tr -d '\n' | awk '
  {
    host = index($0, "\"host\":")
    if (host == 0) exit 1
    rest = substr($0, host)
    metrics = index(rest, "\"metrics\":")
    if (metrics == 0) exit 1
    rest = substr(rest, metrics)
    key = "\"cpu_percent\":"
    at = index(rest, key)
    if (at == 0) exit 1
    rest = substr(rest, at + length(key))
    sub(/^[[:space:]]*/, "", rest)
    if (rest !~ /^[0-9]/) exit 1
    match(rest, /^[0-9]+(\.[0-9]+)?/)
    print substr(rest, RSTART, RLENGTH)
  }
') || fail 'host cpu_percent is missing or null'
host_mem=$(printf '%s' "$body" | tr -d '\n' | awk '
  {
    host = index($0, "\"host\":")
    if (host == 0) exit 1
    rest = substr($0, host)
    metrics = index(rest, "\"metrics\":")
    if (metrics == 0) exit 1
    rest = substr(rest, metrics)
    key = "\"memory_bytes\":"
    at = index(rest, key)
    if (at == 0) exit 1
    rest = substr(rest, at + length(key))
    sub(/^[[:space:]]*/, "", rest)
    if (rest !~ /^[0-9]/) exit 1
    match(rest, /^[0-9]+/)
    print substr(rest, RSTART, RLENGTH)
  }
') || fail 'host memory_bytes is missing or null'
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

body=$(fetch)
box_mem=$(printf '%s' "$body" | tr -d '\n' | awk '
  {
    in_box = 0
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
