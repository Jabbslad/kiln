#!/bin/sh
# Point this clone at repository hooks under .githooks (pre-push runs scripts/check.sh).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  printf 'install-hooks: not inside a git repository\n' >&2
  exit 1
fi

git config core.hooksPath .githooks
printf 'install-hooks: core.hooksPath=.githooks (pre-push runs scripts/check.sh)\n'
