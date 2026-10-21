#!/bin/sh
# Host-side dashboard metrics scripts must run with dark-forge tools only (no jq):
# bash, curl, sqlite3, and coreutils (cat, dirname, grep, mktemp, rm, tr).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-deploy-host-path: %s\n' "$*" >&2
  exit 1
}

host_bin=$(mktemp -d)
trap 'rm -rf "$host_bin"' EXIT

for name in bash cat curl dirname grep mktemp rm sqlite3 tr; do
  path=$(command -v "$name") || fail "missing host tool: $name"
  ln -sf "$path" "$host_bin/$name"
done

export DEPLOY_HOST_PATH="$host_bin"
export DEPLOY_HOST_PATH_FORBID_JQ=1

sh "$root/scripts/test-check-dashboard-metrics.sh" \
  || fail 'host dashboard metrics scripts failed on deploy-host PATH'

printf 'deploy-host path tests ok\n'
