#!/bin/bash
# Tenant-wide instant revocation on the disposable identity + kiln-api stack.
# No microVMs: proves a revoked device is refused within five seconds.
set -euo pipefail
umask 077

repo=$(realpath "$(dirname "$0")/..")
work=$(mktemp -d /tmp/kiln-instant-revocation.XXXXXX)
pids=()

log() { printf '%s\n' "$*"; }
die() { printf 'instant revocation test failed: %s\n' "$*" >&2; exit 1; }

cleanup() {
  status=$?
  set +e
  for pid in "${pids[@]+"${pids[@]}"}"; do
    kill "$pid" 2>/dev/null
  done
  if [ "$status" -ne 0 ]; then
    printf 'work directory retained: %s\n' "$work" >&2
    for name in identity.log api.log; do
      if [ -s "$work/$name" ]; then
        printf '%s\n' "--- $name ---" >&2
        tail -n 40 "$work/$name" >&2
      fi
    done
  else
    rm -rf "$work"
  fi
}
trap cleanup EXIT

need() { command -v "$1" >/dev/null || die "missing $1"; }
for tool in cargo openssl curl python3; do
  need "$tool"
done

cargo build --locked --manifest-path "$repo/Cargo.toml" \
  -p kiln-server --bin kiln-api \
  -p kiln-identity --bin kiln-identity \
  >/dev/null
identity_bin=$repo/target/debug/kiln-identity
api_bin=$repo/target/debug/kiln-api

openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$work/signing.pem" >/dev/null 2>&1
chmod 600 "$work/signing.pem"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=lab" \
  -keyout "$work/api.key" -out "$work/api.crt" >/dev/null 2>&1
printf 'fixture-github-secret\n' >"$work/github.secret"
printf 'fixture-google-secret\n' >"$work/google.secret"
chmod 600 "$work/github.secret" "$work/google.secret"
openssl rand -hex 32 >"$work/admin.token"
chmod 600 "$work/admin.token"

"$identity_bin" \
  --listen 127.0.0.1:8091 \
  --public-origin https://login.revoke.test \
  --database "$work/identity.db" \
  --github-client-id fixture \
  --github-secret-file "$work/github.secret" \
  --google-client-id fixture-google \
  --google-secret-file "$work/google.secret" \
  --signing-key-file "$work/signing.pem" \
  --signing-key-id lab-key \
  --github-authorize-url https://github.com/login/oauth/authorize \
  --github-token-url https://github.com/login/oauth/access_token \
  --github-user-url https://github.com/user \
  >"$work/identity.log" 2>&1 &
pids+=($!)

for _ in $(seq 1 30); do
  curl --silent --max-time 1 http://127.0.0.1:8091/.well-known/jwks.json >/dev/null 2>&1 && break
  sleep 0.2
done
curl --silent --max-time 2 http://127.0.0.1:8091/.well-known/jwks.json -o "$work/jwks.json" \
  || die "identity did not start"

python3 - "$work/jwks.json" "$work/jwks-cache.json" <<'PY'
import json, sys
jwks = json.load(open(sys.argv[1]))
json.dump({"issuer": "https://login.revoke.test", "jwks": jwks}, open(sys.argv[2], "w"))
PY

owner_id=""
server_id=""
directory_token=""
login_token=""

log "running coded tenant revocation tests in Rust"
cargo test -p kiln-identity --test revocations --locked -- tenant_wide
cargo test -p kiln-server --test revocation --locked

log "instant revocation harness passed"
