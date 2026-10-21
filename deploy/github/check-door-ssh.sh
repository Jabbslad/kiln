#!/bin/bash
# After deploy, the public door's SSH route must answer and reject a request
# that carries no credential. A failed connection is not a rejection.
set -euo pipefail

fail() { printf 'door-ssh: %s\n' "$*" >&2; exit 1; }

box_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
host_name=cli.dark-forge.dev
ca=${KILN_DOOR_CA:-/etc/kiln/ca.crt}

if [ -n "${KILN_DOOR_SSH_URL:-}" ]; then
  url=$KILN_DOOR_SSH_URL
  resolve=${KILN_DOOR_RESOLVE:-}
else
  [ -f /etc/kiln/door.env ] || fail 'door ssh route is not reachable'
  listen=$(awk -F= '$1 == "KILN_DOOR_LISTEN" { print $2; exit }' /etc/kiln/door.env)
  case $listen in
    127.0.0.1:[1-9]*) ;;
    *) fail 'door ssh route is not reachable' ;;
  esac
  port=${listen##*:}
  case $port in
    *[!0-9]*) fail 'door ssh route is not reachable' ;;
  esac
  url="https://${host_name}:${port}/v1/boxes/${box_id}/ssh"
  resolve="${host_name}:${port}:127.0.0.1"
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

curl_args=(
  --silent
  --show-error
  --proto '=https'
  --proto-redir '=https'
  --connect-timeout 5
  --max-time 10
  --cacert "$ca"
  --output "$body"
  --write-out '%{http_code}'
  --header 'Connection: upgrade'
  --header 'Upgrade: kiln-ssh'
  --header 'x-kiln-ssh-key: ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
)
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
printf 'door ssh unauthenticated: 401\n'
