#!/bin/bash
# Create one box, run a command, and delete the box even when the command fails.
set -euo pipefail
umask 077

fail() { printf 'smoke: %s\n' "$*" >&2; exit 1; }

address=${1:?usage: smoke.sh ADDRESS}
kiln=${KILN_BIN:-/opt/kiln/bin/kiln}
[ -x "$kiln" ] || fail 'kiln client is not installed'
printf '%s\n' "$address" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$' \
  || fail 'address must be an IPv4 literal'

work=$(mktemp -d)
config=$work/profiles.json
box_id=
delete_rc=0
deleted=false

delete_box() {
  if [ -z "$box_id" ] || [ "$deleted" = true ]; then
    return 0
  fi
  "$kiln" --config "$config" --profile default delete "$box_id" || delete_rc=$?
  deleted=true
  return "$delete_rc"
}

# shellcheck disable=SC2317
cleanup() {
  status=$?
  delete_box || status=1
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT

"$kiln" --config "$config" profile add default \
  --url "https://${address}:8443" \
  --token-file /etc/kiln/admin.token \
  --ca-file /etc/kiln/ca.crt

create_rc=0
create_out=$(
  "$kiln" --config "$config" --profile default --json \
    create --template ubuntu-4g --name kiln-smoke
) || create_rc=$?
box_id=$(printf '%s' "$create_out" | tr -d '[:space:]' | awk '
  {
    marker = "\"result\":{"
    at = index($0, marker)
    if (at == 0) exit 1
    rest = substr($0, at)
    type_key = "\"type\":\""
    type_at = index(rest, type_key)
    if (type_at == 0) exit 1
    type_rest = substr(rest, type_at + length(type_key))
    type_end = index(type_rest, "\"")
    if (type_end == 0 || substr(type_rest, 1, type_end - 1) != "box") exit 1
    id_key = "\"id\":\""
    id_at = index(rest, id_key)
    if (id_at == 0) exit 1
    id_rest = substr(rest, id_at + length(id_key))
    id_end = index(id_rest, "\"")
    if (id_end == 0) exit 1
    id = substr(id_rest, 1, id_end - 1)
    if (id !~ /^[0-9a-f]{32}$/) exit 1
    printf "%s", id
  }
') || box_id=

exec_rc=0
exec_out=
if [ -n "$box_id" ]; then
  # Human exec writes guest stdout. --json keeps it inside the operation document.
  exec_out=$(
    "$kiln" --config "$config" --profile default \
      exec "$box_id" -- /bin/sh -c 'printf kiln-smoke-ok'
  ) || exec_rc=$?
fi

metrics_rc=0
metrics_log=$work/self-check.log
token_file=${KILN_ADMIN_TOKEN_FILE:-/etc/kiln/admin.token}
ca_file=${KILN_CA_FILE:-/etc/kiln/ca.crt}
if [ -n "$box_id" ] && [ -r "$token_file" ] && [ -r "$ca_file" ]; then
  kiln_api=${KILN_API_BIN:-/usr/local/libexec/kiln-api}
  : >"$metrics_log"
  "$kiln_api" self-check --address "$address" \
    --require-launch-timing kiln-smoke >>"$metrics_log" 2>&1 || metrics_rc=$?
  cat "$metrics_log"
fi

delete_box || true

status=0
if [ "$create_rc" -ne 0 ] || [ -z "$box_id" ]; then
  printf 'smoke: box create failed\n' >&2
  status=1
fi
if [ -n "$box_id" ] && [ "$exec_rc" -ne 0 ]; then
  printf 'smoke: box exec failed\n' >&2
  status=1
fi
if [ -n "$box_id" ] && [ "$exec_rc" -eq 0 ] && [ "$exec_out" != "kiln-smoke-ok" ]; then
  printf 'smoke: box exec output mismatch\n' >&2
  status=1
fi
if [ "$delete_rc" -ne 0 ]; then
  printf 'smoke: box delete failed\n' >&2
  status=1
fi
if [ "$metrics_rc" -ne 0 ]; then
  printf 'smoke: kiln-api self-check failed (exit %s)\n%s\n' \
    "$metrics_rc" "$(cat "$metrics_log" 2>/dev/null || true)" >&2
  status=1
fi
exit "$status"
