#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-deploy-standin-final-checks: %s\n' "$*" >&2
  exit 1
}

workflow=$root/.github/workflows/build.yml
grep -q '^  deploy-final-checks:' "$workflow" \
  || fail 'build.yml must define deploy-final-checks'
grep -q 'deploy_final' "$workflow" \
  || fail 'changes job must expose deploy_final output'
grep -q 'sh scripts/deploy-standin-final-checks.sh' "$workflow" \
  || fail 'deploy-final-checks must run deploy-standin-final-checks.sh'
grep -q 'KILN_STANDIN_FAULT_INJECT=1 sh scripts/deploy-standin-final-checks.sh' "$workflow" \
  || fail 'deploy-final-checks must run fault injection'

deploy=$root/deploy/github/deploy.sh
grep -q 'KILN_DEPLOY_STANDIN' "$deploy" \
  || fail 'deploy.sh must support KILN_DEPLOY_STANDIN with KILN_DEPLOY_DIST'
remote=$root/deploy/github/install-remote.sh
grep -q 'KILN_SKIP_SMOKE' "$remote" \
  || fail 'install-remote.sh must honor KILN_SKIP_SMOKE'

printf 'test-deploy-standin-final-checks: ok\n'
