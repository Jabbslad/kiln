#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-verify-dashboard-host-metrics: %s\n' "$*" >&2
  exit 1
}

verify=$root/deploy/github/verify-dashboard-host-metrics.sh
[ -x "$verify" ] || chmod +x "$verify"
command -v jq >/dev/null 2>&1 || fail 'jq is required on the runner'

gateway_stamped='{"boxes":[{"state":"stopped","metrics":{"cpu_percent":null,"memory_bytes":1}}],"checkpoints":[],"host":{"metrics":{"cpu_percent":0.42,"memory_bytes":5000,"disk_bytes":null},"name":"local","state":"online"},"observed_at":"2026-10-21T00:00:00Z","operations":[],"schema_version":1,"templates":[],"workspace":"local"}'
printf '%s' "$gateway_stamped" | bash "$verify" >/dev/null \
  || fail 'gateway-stamped body must pass jq host.metrics verification'

printf '{"host":{"metrics":{"cpu_percent":null,"memory_bytes":1000}},"boxes":[]}' | bash "$verify" >/dev/null 2>&1 \
  && fail 'null host cpu must fail'

printf 'verify-dashboard-host-metrics tests ok\n'
