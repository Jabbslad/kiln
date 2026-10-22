#!/bin/sh
# Fail if a release or tag-path binary enabled kiln-server's
# loopback-identity-ca feature, or if its bytes contain that CA path.
# Pass the cargo --message-format=json log from the build that produced
# the binaries, then the binary paths (target/release and the packed archive).
set -eu

fail() {
  printf 'release-trust: %s\n' "$*" >&2
  exit 1
}

json=
while [ $# -gt 0 ]; do
  case "$1" in
    --cargo-json)
      shift
      [ $# -gt 0 ] || fail '--cargo-json needs a file'
      json=$1
      shift
      ;;
    --)
      shift
      break
      ;;
    -*)
      fail "unknown argument: $1"
      ;;
    *)
      break
      ;;
  esac
done

[ -n "$json" ] || fail 'pass --cargo-json from the release cargo build'
[ -f "$json" ] || fail "missing cargo json $json"
[ $# -gt 0 ] || fail 'no binaries'
command -v jq >/dev/null 2>&1 || fail 'jq is required'

artifacts=$(jq -s '[.[] | select(.reason == "compiler-artifact")] | length' "$json") \
  || fail 'could not read cargo json'
[ "$artifacts" -gt 0 ] || fail 'cargo json has no compiler artifacts'

printf 'release-trust: command sh scripts/check-release-trust.sh --cargo-json %s' "$json"
for bin in "$@"; do
  printf ' %s' "$bin"
done
printf '\n'

enabled=$(
  jq -s -r '[.[] | select(.reason == "compiler-artifact") | .features[]? | select(. == "loopback-identity-ca")] | unique | .[]' "$json"
) || fail 'could not read cargo features'
if [ -n "$enabled" ]; then
  fail 'release cargo build enabled loopback-identity-ca'
fi

for bin in "$@"; do
  [ -f "$bin" ] || fail "missing binary $bin"
  if grep -a -F -q 'loopback-identity-ca.pem' "$bin"; then
    fail "$bin contains loopback-identity-ca.pem"
  fi
done

printf 'release-trust: ok\n'
printf 'release-trust: cargo artifacts %s\n' "$artifacts"
printf 'release-trust: loopback-identity-ca feature: absent\n'
for bin in "$@"; do
  printf 'release-trust: no loopback-identity-ca.pem in %s\n' "$bin"
done
