#!/bin/sh
# Host-side deploy scripts must run with only tools install-remote already
# hard-requires on an upgraded host: bash, curl, awk, and coreutils (cat, dirname, grep, tr).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-deploy-host-path: %s\n' "$*" >&2
  exit 1
}

host_bin=$(mktemp -d)
trap 'rm -rf "$host_bin"' EXIT

for name in awk bash cat curl dirname grep tr; do
  path=$(command -v "$name") || fail "missing host tool: $name"
  ln -sf "$path" "$host_bin/$name"
done

export DEPLOY_HOST_PATH="$host_bin"
export DEPLOY_HOST_PATH_FORBID_JQ=1

sh "$root/scripts/test-check-dashboard-metrics.sh" \
  || fail 'host dashboard metrics scripts failed on deploy-host PATH'

printf 'deploy-host path tests ok\n'
