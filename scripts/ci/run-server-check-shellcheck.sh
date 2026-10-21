#!/bin/sh
# server-check shellcheck list from .github/workflows/build.yml
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

shellcheck scripts/fetch-firecracker.sh scripts/install-caddy.sh scripts/check-bootstrap-pins.sh scripts/test-bootstrap-pins.sh scripts/test-caddy-endings.sh images/build-ubuntu.sh deploy/install.sh deploy/bootstrap/install.sh \
  deploy/enroll-dark-forge.sh \
  deploy/github/guard.sh deploy/github/ssh.sh deploy/github/deploy.sh \
  deploy/github/require-release.sh \
  deploy/github/install-remote.sh deploy/github/install-identity.sh deploy/github/smoke.sh \
  deploy/github/check-dashboard-bundle.sh \
  scripts/test-check-dashboard-bundle.sh \
  deploy/github/diagnose.sh deploy/github/diagnose-run.sh \
  scripts/test-listener-restart.sh \
  scripts/test-wait-for-operations-journal-v2.sh \
  scripts/test-identity-upgrade.sh \
  scripts/test-installer-release.sh \
  scripts/stamp-release-version.sh scripts/stamp-release-dist.sh scripts/reuse-server-check.sh \
  scripts/wait-main-run.sh scripts/test-wait-main-run.sh \
  scripts/tag-path-dry-run.sh scripts/test-tag-path-dry-run.sh \
  scripts/releasable.sh scripts/test-releasable.sh \
  scripts/next-release-tag.sh scripts/test-next-release-tag.sh \
  scripts/plan-release.sh scripts/test-plan-release.sh \
  scripts/check-stamped-clients.sh scripts/test-check-stamped-clients.sh \
  scripts/check-dropped-platform.sh scripts/test-check-dropped-platform.sh \
  scripts/test-release-budget.sh \
  scripts/check-feature-map.sh scripts/test-feature-map-guard.sh \
  scripts/fmt.sh scripts/test-fmt.sh scripts/check.sh scripts/test-check.sh \
  scripts/ci/run-docs-check.sh scripts/ci/run-server-check.sh scripts/ci/run-server-check-shellcheck.sh \
  scripts/ci/run-dashboard-check.sh scripts/ci/run-dashboard-tests.sh \
  scripts/ci/run-client-check.sh scripts/ci/run-client-tests.sh \
  scripts/ci/run-feature-map-guard.sh scripts/ci/ensure-socket-tools.sh
