#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
fail() { printf 'test-proof-door-ssh-caddy: %s\n' "$*" >&2; exit 1; }

before=$root/deploy/proof/door-ssh-caddy/before-curl.txt
after=$root/deploy/proof/door-ssh-caddy/after-curl.txt
[ -f "$before" ] || fail 'missing before-curl.txt; run sh scripts/proof-door-ssh-caddy.sh'
[ -f "$after" ] || fail 'missing after-curl.txt; run sh scripts/proof-door-ssh-caddy.sh'
grep -q 'HTTP 502' "$before" || fail 'before transcript must show HTTP 502'
grep -q 'HTTP 401' "$after" || fail 'after transcript must show HTTP 401'
grep -q 'audit ssh outcome=unauthorized' "$after" || fail 'after transcript must include an audit ssh line'
grep -q 'caller=anonymous' "$after" || fail 'after transcript must name caller=anonymous'

command -v caddy >/dev/null 2>&1 || bash scripts/install-caddy.sh
cargo test --locked -p kiln-server --test door_caddy kiln_ssh_upgrade_through_caddy_needs_http11_upstream -- --exact --nocapture

printf 'test-proof-door-ssh-caddy: ok\n'
