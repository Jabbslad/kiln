#!/bin/sh
# docs-check "Repository checks" from .github/workflows/build.yml (opt-in via check.sh).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo xtask test-ci-docs-only
cargo xtask test-release-version
cargo xtask test-release
cargo xtask test-install
cargo xtask test-bootstrap
cargo xtask test-close-public-door
cargo xtask test-caddy-door
cargo xtask test-deploy-workflow
cargo xtask test-reuse-release
cargo xtask test-merge-queue-tested
cargo xtask test-repo-guard
bash scripts/test-listener-restart.sh
bash scripts/test-wait-for-operations-journal-v2.sh
bash scripts/test-identity-upgrade.sh
sh scripts/test-caddy-endings.sh
sh scripts/test-bootstrap-pins.sh
sh scripts/test-installer-release.sh
sh scripts/test-wait-main-run.sh
sh scripts/test-tag-path-dry-run.sh
sh scripts/test-check-dashboard-bundle.sh
cargo test -p kiln-server --test dashboard_self_check --locked
sh scripts/test-releasable.sh
sh scripts/test-next-release-tag.sh
sh scripts/test-plan-release.sh
sh scripts/test-check-stamped-clients.sh
sh scripts/test-release-budget.sh
sh scripts/test-check-dropped-platform.sh
sh scripts/test-feature-map-guard.sh
sh scripts/test-check-pr-proof.sh
sh scripts/test-operations-journal-version-sync.sh
sh scripts/test-fmt.sh
sh scripts/test-check.sh
