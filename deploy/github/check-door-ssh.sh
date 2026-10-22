#!/bin/bash
# After deploy, the public door's SSH route must answer through Caddy on 443
# with HTTP/1.1 Upgrade: kiln-ssh. A Caddy 502 means the upgrade never reached
# the door. A failed connection is not a rejection.
set -euo pipefail

fail() { printf 'door-ssh: %s\n' "$*" >&2; exit 1; }

box_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
host_name=cli.dark-forge.dev

if [ -n "${KILN_DOOR_SSH_URL:-}" ]; then
  url=$KILN_DOOR_SSH_URL
  resolve=${KILN_DOOR_RESOLVE:-}
else
  systemctl is-active --quiet kiln-door.service \
    || fail 'door ssh route is not reachable'
  url="https://${host_name}/v1/boxes/${box_id}/ssh"
  resolve=${KILN_DOOR_RESOLVE:-}
fi

case $url in
  https://*/v1/boxes/"${box_id}"/ssh) ;;
  *) fail 'door ssh url is not the ssh route' ;;
esac
case $url in
  *\?* | *@*) fail 'door ssh url must not carry a query or credential' ;;
esac

body=$(mktemp)
trap 'rm -f "$body"' EXIT

audit_since=${KILN_DOOR_AUDIT_SINCE:-}
if [ -z "$audit_since" ]; then
  audit_since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
fi

curl_args=(
  --silent
  --show-error
  --http1.1
  --proto '=https'
  --proto-redir '=https'
  --connect-timeout 5
  --max-time 10
  --output "$body"
  --write-out '%{http_code}'
  --header 'Connection: Upgrade'
  --header 'Upgrade: kiln-ssh'
  --header 'x-kiln-ssh-key: ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
)
if [ -n "${KILN_DOOR_CA:-}" ]; then
  curl_args+=(--cacert "$KILN_DOOR_CA")
fi
if [ -n "$resolve" ]; then
  curl_args+=(--resolve "$resolve")
fi

code=$(curl "${curl_args[@]}" "$url") || fail 'door ssh route is not reachable'
[ "$code" = 401 ] || fail "unauthenticated door ssh returned ${code:-none}"
grep -q '"code":"unauthorized"' "$body" \
  || fail 'unauthenticated door ssh body was not a rejection'
if grep -q -E 'Bearer |admin\.token|Authorization' "$body"; then
  fail 'door ssh rejection included a credential'
fi

journalctl -u kiln-door.service --since "$audit_since" --no-pager \
  | grep -q "audit ssh outcome=unauthorized box_id=${box_id}" \
  || fail 'door did not audit the ssh attempt'

printf 'door ssh unauthenticated: 401\n'
