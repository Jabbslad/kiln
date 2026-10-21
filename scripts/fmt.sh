#!/bin/sh
# Apply the same formatting CI checks: rustfmt and Prettier on web/.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

mode=write
if [ "${1:-}" = --check ]; then
  mode=check
  shift
fi
if [ $# -gt 0 ]; then
  printf 'fmt: unknown argument: %s\n' "$1" >&2
  exit 2
fi

if [ "$mode" = check ]; then
  cargo fmt --all -- --check
else
  cargo fmt --all
fi

(
  cd web
  if [ ! -x node_modules/.bin/prettier ]; then
    npm ci
  fi
  if [ "$mode" = check ]; then
    npx prettier --check .
  else
    npx prettier --write .
  fi
)
