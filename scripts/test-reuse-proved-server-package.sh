#!/bin/sh
# Guard: PRs publish a proved package-server artifact; main can download it after merge.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow=$root/.github/workflows/build.yml
tag_script=$root/scripts/tag-path-dry-run.sh

fail() {
  printf 'test-reuse-proved-server-package: %s\n' "$*" >&2
  exit 1
}

grep -q 'dist-proved-package-server' "$tag_script" \
  || fail 'tag-path-dry-run must stage dist-proved-package-server on pull requests'
grep -q 'scripts/package-server-release.sh' "$tag_script" \
  || fail 'tag-path-dry-run must call package-server-release.sh on pull requests'
grep -q 'SKIP_RELEASE_BUILD=1' "$tag_script" \
  || fail 'tag-path-dry-run must skip rebuilding release binaries for proved package-server'

block=$(
  awk '/^  tag-path-dry-run:/{flag=1} flag && /^  [a-z].*:/ && !/^  tag-path-dry-run:/{exit} flag{print}' \
    "$workflow"
)
printf '%s\n' "$block" | grep -q 'name: package-server' \
  || fail 'tag-path-dry-run must upload package-server on pull requests'

printf '%s\n' "$block" | grep -q "github.event_name == 'pull_request'" \
  || fail 'package-server upload must be pull_request only'

server=$(
  awk '/^  server-package:/{flag=1} flag && /^  [a-z].*:/ && !/^  server-package:/{exit} flag{print}' \
    "$workflow"
)
printf '%s\n' "$server" | grep -q 'gh run download' \
  || fail 'server-package must download a proved package-server artifact'
printf '%s\n' "$server" | grep -q 'proof_run_id' \
  || fail 'server-package reuse must read proof_run_id from merge-queue-tested'
printf '%s\n' "$server" | grep -q 'continue-on-error: true' \
  || fail 'server-package reuse must fail open to a full rebuild'

grep -q 'scripts/test-reuse-proved-server-package.sh' "$root/scripts/check-docs.sh" \
  || fail 'check-docs must run test-reuse-proved-server-package'
