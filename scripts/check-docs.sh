#!/bin/sh
# docs-check "Repository checks" from .github/workflows/build.yml (opt-in via check.sh).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo xtask test-ci-docs-only
cargo xtask test-ci-job-paths
sh scripts/test-dashboard-npm-ci.sh
sh scripts/test-playwright-ci-cache.sh
cargo xtask test-release-version
cargo xtask test-release
cargo xtask test-install
cargo xtask test-bootstrap
cargo xtask test-close-public-door
cargo xtask test-caddy-door
cargo xtask test-deploy-workflow
cargo xtask test-reuse-release
cargo xtask test-merge-queue-tested
sh scripts/test-reuse-proved-server-package.sh
cargo xtask test-repo-guard
bash scripts/test-listener-restart.sh
bash scripts/test-wait-for-operations-journal-v2.sh
bash scripts/test-identity-upgrade.sh
sh scripts/test-caddy-endings.sh
sh scripts/test-bootstrap-pins.sh
sh scripts/test-installer-release.sh
sh scripts/test-wait-main-run.sh
sh scripts/test-tag-path-dry-run.sh
sh scripts/test-tag-path-pr-stamp-order.sh
sh scripts/test-tag-path-parallel.sh
sh scripts/test-ci-budget.sh
sh scripts/test-main-push-proved-package-stamp.sh
sh scripts/test-deploy-host-tools.sh
sh scripts/test-release-json-field.sh
sh scripts/test-check-dashboard-bundle.sh
cargo test -p kiln-server --test dashboard_self_check --locked
sh scripts/test-releasable.sh
sh scripts/test-next-release-tag.sh
sh scripts/test-plan-release.sh
sh scripts/test-check-stamped-clients.sh
sh scripts/test-release-budget.sh
sh scripts/test-verify-release-attestations.sh
sh scripts/test-check-dropped-platform.sh
sh scripts/test-feature-map-guard.sh
sh scripts/test-check-pr-proof.sh
sh scripts/test-operations-journal-version-sync.sh
sh scripts/test-tenant-isolation.sh
sh scripts/test-release-notes.sh
sh scripts/test-fmt.sh
sh scripts/test-check.sh
sh scripts/test-server-check-nextest.sh
sh scripts/test-server-check-hang-evidence.sh
