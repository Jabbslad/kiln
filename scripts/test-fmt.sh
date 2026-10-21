#!/bin/sh
# fmt.sh must leave the tree unchanged; CI check commands must pass afterward.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-fmt: %s\n' "$*" >&2
  exit 1
}

(
  cd web
  npm ci
)

sh "$root/scripts/fmt.sh"

if ! git diff --exit-code; then
  fail 'fmt.sh changed tracked files; run it and commit'
fi

cargo fmt --all -- --check

(
  cd web
  npx prettier --check .
)

printf 'fmt guard passed\n'
