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
box_id=$(printf '%s\n' "$create_out" | python3 -c '
import json, re, sys
raw = sys.stdin.read()
if not raw.strip():
    raise SystemExit(1)
try:
    op = json.loads(raw)
except json.JSONDecodeError:
    raise SystemExit(1)
result = op.get("result") or {}
box_id = result.get("id", "") if result.get("type") == "box" else ""
if re.fullmatch(r"[0-9a-f]{32}", box_id) is None:
    raise SystemExit(1)
print(box_id)
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
exit "$status"
