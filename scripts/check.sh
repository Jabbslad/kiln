#!/bin/sh
# Fast local checks before push. See .github/workflows/build.yml for full CI.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# Pre-push and local runs have no GITHUB_ACTIONS. Force the low-memory path
# before clippy so an agent VM cannot launch three nextest shards after the
# balloon deflates and MemAvailable looks large. CI sets GITHUB_ACTIONS and
# keeps the parallel runner when the machine has the RAM.
if [ -z "${GITHUB_ACTIONS:-}" ]; then
  export KILN_NEXTEST_SERIAL=1
fi
if [ "${KILN_NEXTEST_SERIAL:-}" = 1 ]; then
  : "${CARGO_PROFILE_DEV_DEBUG:=0}"
  : "${CARGO_PROFILE_TEST_DEBUG:=0}"
  export CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG
fi

docs=0
release=0
while [ $# -gt 0 ]; do
  case "$1" in
    --docs-check) docs=1 ;;
    --release) release=1 ;;
    -h | --help)
      cat <<'EOF'
Usage: sh scripts/check.sh [--docs-check] [--release]

Default (fast; also run by .githooks/pre-push when hooks are installed):
  fmt check, server-check (cargo fmt/clippy/test), feature-map-guard

  --docs-check  Also run the docs-check repository script bundle (slow; opt-in).
  --release     Also run tag-path-dry-run (needs GH_TOKEN / gh auth login).

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

run_server_check() {
  cargo fmt --all -- --check &&
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings &&
  if [ "${KILN_NEXTEST_SERIAL:-}" = 1 ]; then
    sh scripts/test-nextest-memory-cap.sh
  else
    sh scripts/run-workspace-nextest.sh
  fi &&
  if command -v shellcheck >/dev/null 2>&1; then
    shellcheck -S error scripts/check.sh scripts/test-check.sh scripts/check-docs.sh \
      scripts/run-workspace-nextest.sh scripts/nextest-low-memory.sh \
      scripts/test-nextest-memory-cap.sh scripts/test-nextest-cap-verdict.sh \
      scripts/test-tag-path-image-once.sh
  else
    printf 'shellcheck not installed, skipped\n'
  fi
}

run_check server-check run_server_check

run_feature_map_guard() {
  base=${PR_BASE_SHA:-}
  if [ -z "$base" ]; then
    git fetch origin main 2>/dev/null || true
    if git rev-parse --verify origin/main >/dev/null 2>&1; then
      base=$(git merge-base HEAD origin/main 2>/dev/null || echo origin/main)
    else
      base=origin/main
    fi
  fi
  git diff --name-only --no-renames "$base" HEAD \
    | sh scripts/check-feature-map.sh
}

run_check feature-map-guard run_feature_map_guard

run_check batch-merge sh scripts/test-batch-merge.sh

if [ "$docs" = 1 ]; then
  run_check docs-check sh scripts/check-docs.sh
fi

if [ "$release" = 1 ]; then
  repo=${GH_REPO:-Jabbslad/kiln}
  export GH_REPO="$repo"
  if [ -z "${GH_TOKEN:-}" ] && command -v gh >/dev/null 2>&1; then
    GH_TOKEN=$(gh auth token 2>/dev/null || true)
  fi
  export GH_TOKEN
  if [ -z "${GH_TOKEN:-}" ]; then
    printf 'FAILED: tag-path-dry-run (set GH_TOKEN or run gh auth login)\n' >&2
    exit 1
  fi
  export GITHUB_EVENT_NAME=pull_request
  run_check tag-path-dry-run sh scripts/tag-path-dry-run.sh
fi

printf 'check: all checks passed\n'
