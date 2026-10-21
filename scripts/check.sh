#!/bin/sh
# Local equivalent of required CI checks (see .github/workflows/build.yml).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

release=0
while [ $# -gt 0 ]; do
  case "$1" in
    --release) release=1 ;;
    -h | --help)
      cat <<'EOF'
Usage: sh scripts/check.sh [--release]

Runs the same checks as required CI jobs before you push:
  fmt (check mode), docs-check, server-check, dashboard, client (this OS),
  and feature-map-guard.

  --release  Also run tag-path-dry-run (needs GH_TOKEN; uses gh auth token when set).

Exits non-zero on the first failure and prints which check failed.
EOF
      exit 0
      ;;
    *)
      printf 'check: unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
  shift
done

if [ -z "${GITHUB_TOKEN:-}" ] && command -v gh >/dev/null 2>&1; then
  GITHUB_TOKEN=$(gh auth token 2>/dev/null || true)
  export GITHUB_TOKEN
fi

run_check() {
  name=$1
  shift
  printf '==> %s\n' "$name"
  if "$@"; then
    printf 'ok: %s\n' "$name"
  else
    printf 'FAILED: %s\n' "$name" >&2
    exit 1
  fi
}

run_check fmt sh scripts/fmt.sh --check
run_check install-caddy bash scripts/install-caddy.sh
run_check socket-tools bash scripts/ci/ensure-socket-tools.sh
run_check docs-check bash scripts/ci/run-docs-check.sh
run_check server-check bash scripts/ci/run-server-check.sh
run_check dashboard bash scripts/ci/run-dashboard-check.sh
run_check client bash scripts/ci/run-client-check.sh
run_check feature-map-guard bash scripts/ci/run-feature-map-guard.sh

if [ "$release" = 1 ]; then
  repo=${GH_REPO:-Jabbslad/kiln}
  export GH_REPO="$repo"
  if [ -z "${GH_TOKEN:-}" ]; then
    printf 'FAILED: tag-path-dry-run (set GH_TOKEN or run gh auth login)\n' >&2
    exit 1
  fi
  export GH_TOKEN
  run_check tag-path-dry-run sh scripts/tag-path-dry-run.sh
fi

printf 'check: all checks passed\n'
