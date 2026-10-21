#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

output=$(mktemp)
trap 'rm -f "$output"' EXIT

sh "$root/scripts/prove-create-wait-cli.sh" > "$output" 2>&1

grep -Fq '=== before: kiln create --json (202 + client poll) ===' "$output" ||
  {
    cat "$output"
    exit 1
  }
grep -Fq '=== after: kiln create --wait --json (200, no client poll) ===' "$output" ||
  {
    cat "$output"
    exit 1
  }
grep -Fq '"state":"succeeded"' "$output" || grep -Fq '"state": "succeeded"' "$output" ||
  {
    cat "$output"
    exit 1
  }
grep -Fq 'create_posts_with_wait=1' "$output" ||
  {
    cat "$output"
    exit 1
  }

printf 'prove-create-wait-cli ok\n'
