#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-deploy-standin-ssh-secret-guard: %s\n' "$*" >&2
  exit 1
}

workflow=$root/.github/workflows/build.yml
block=$(awk '/^  deploy-final-checks:$/,/^  tag-path-dry-run:$/' "$workflow")
printf '%s\n' "$block" | grep -q 'upload-artifact' \
  && fail 'deploy-final-checks must not upload artifacts (stand-in SSH key lives under mktemp)'

if grep -E 'cat[^
]*"\$work/standin-id"|cat[^
]*standin-id[^.p]' scripts/deploy-standin-host.sh 2>/dev/null; then
  fail 'deploy-standin-host must not cat the private key file'
fi

grep -q 'OPENSSH PRIVATE KEY' scripts/deploy-standin-host.sh \
  && fail 'stand-in scripts must not embed private key material'

grep -q 'missing stand-in private key file' scripts/deploy-standin-ssh.sh \
  || fail 'deploy-standin-ssh must not print the private key path on failure'

printf 'test-deploy-standin-ssh-secret-guard: ok\n'
