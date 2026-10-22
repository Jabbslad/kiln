#!/bin/sh
# Disposable-server isolation for tenant-scoped multi-account box boundaries.
# Never runs against dark-forge. Skips until the tenant stack (PRs 1/6–5/6) is in-tree.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-tenant-isolation: %s\n' "$*" >&2
  exit 1
}

tenant_stack_ready() {
  grep -q 'tenants' "$root/crates/kiln-identity/src/store.rs" \
    && grep -q 'tenant_id' "$root/crates/kiln-api/src/auth.rs" \
    && grep -q 'pub role' "$root/crates/kiln-api/src/auth.rs" \
    && grep -q 'tenant_id' "$root/crates/kiln-server/src/auth.rs" \
    && grep -q 'struct Caller' "$root/crates/kiln-server/src/auth.rs" \
    && grep -q 'pub owner:' "$root/crates/kiln-runtime/src/runtime.rs"
}

if ! tenant_stack_ready; then
  printf 'test-tenant-isolation: skip (tenant stack not merged)\n'
  exit 0
fi

cargo test -p kiln-server --test tenant_isolation --locked \
  -- --include-ignored --nocapture \
  || fail 'tenant isolation test failed'

printf 'test-tenant-isolation: ok\n'
