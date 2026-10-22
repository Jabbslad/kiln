#!/bin/sh
# Read one string field from release.json on stdin via typed JSON parsing.
set -eu

field=${1:?usage: read-release-json-field.sh FIELD [--strip-v]}
strip=
if [ "${2:-}" = --strip-v ]; then
  strip=--strip-v
fi

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
exec cargo xtask release-version read-json-field --field "$field" $strip
