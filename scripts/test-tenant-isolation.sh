#!/bin/sh
# Real gateway/Unix-host isolation with disposable metadata. No VM or live service.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-tenant-isolation: %s\n' "$*" >&2
  exit 1
}

cargo test -p kiln-server --test tenant_isolation --locked \
  -- --nocapture "$@" \
  || fail 'tenant isolation test failed'

printf 'test-tenant-isolation: ok\n'
