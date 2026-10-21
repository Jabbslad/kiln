#!/bin/bash
# Fetch the live dashboard JSON from loopback kiln-api. Host-side only (curl).
set -euo pipefail
umask 077

fail() { printf 'dashboard-metrics: %s\n' "$*" >&2; exit 1; }

address=${1:?usage: fetch-dashboard-metrics.sh ADDRESS}
printf '%s\n' "$address" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$' \
  || fail 'address must be an IPv4 literal'
token_file=${KILN_ADMIN_TOKEN_FILE:-/etc/kiln/admin.token}
ca_file=${KILN_CA_FILE:-/etc/kiln/ca.crt}
[ -r "$token_file" ] || fail "missing admin token: ${token_file}"
[ -r "$ca_file" ] || fail "missing CA file: ${ca_file}"
token=$(tr -d '[:space:]' <"$token_file")
[ "${#token}" -eq 64 ] || fail 'admin token must be 64 hex characters'

url="https://${address}:8443/api/v1/dashboard"
curl --fail --silent --show-error \
  --cacert "$ca_file" \
  -H "Authorization: Bearer ${token}" \
  "$url"
