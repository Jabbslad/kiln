#!/bin/sh
# Apply the same formatting CI checks: rustfmt and Prettier on web/.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo fmt --all

(
  cd web
  if [ ! -x node_modules/.bin/prettier ]; then
    npm ci
  fi
  npx prettier --write .
)
