#!/bin/sh
# Body of server-check "Test and lint" (also used by hang-evidence proofs).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

if [ "${KILN_SERVER_CHECK_TEST_AND_LINT_MODE:-}" = hang-probe ]; then
  cargo nextest run --profile ci -p ci-guards hang_probe_sleep --locked
  exit 0
fi

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
NEXTEST_SHARDS=3 sh scripts/run-workspace-nextest.sh
cargo xtask test-release-version
cargo xtask test-release
cargo xtask test-install
cargo xtask test-bootstrap
cargo xtask test-close-public-door
cargo xtask test-caddy-door
cargo xtask test-deploy-workflow
cargo xtask test-ci-docs-only
cargo xtask test-ci-job-paths
sh scripts/test-dashboard-npm-ci.sh
sh scripts/test-playwright-ci-cache.sh
cargo xtask test-reuse-release
cargo xtask test-merge-queue-tested
sh scripts/test-reuse-proved-server-package.sh
cargo xtask test-cancel-on-failure-gate
cargo xtask test-repo-guard
bash scripts/test-listener-restart.sh
sh scripts/test-install-upgrade-failure-message.sh
bash scripts/test-wait-for-operations-journal-v2.sh
bash scripts/test-identity-upgrade.sh
bash scripts/test-login-token.sh
bash scripts/test-login-token-upgrade.sh
sh scripts/test-caddy-endings.sh
sh scripts/test-proof-door-ssh-caddy.sh
sh scripts/test-bootstrap-pins.sh
sh scripts/test-installer-release.sh
sh scripts/test-wait-main-run.sh
sh scripts/test-batch-merge.sh
sh scripts/test-tag-path-dry-run.sh
sh scripts/test-tag-path-pr-stamp-order.sh
sh scripts/test-deploy-host-tools.sh
sh scripts/test-release-json-field.sh
sh scripts/test-check-dashboard-bundle.sh
cargo test -p kiln-server --test dashboard_self_check --locked
sh scripts/test-check-installed-versions.sh
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
sh scripts/test-pre-push-hook.sh
sh scripts/test-ssh-multiplex.sh
bash -n scripts/qa/swarm.sh
bash scripts/qa/swarm.sh --help
shellcheck scripts/fetch-firecracker.sh scripts/install-caddy.sh scripts/check-bootstrap-pins.sh scripts/test-bootstrap-pins.sh scripts/test-caddy-endings.sh images/build-ubuntu.sh deploy/install.sh deploy/bootstrap/install.sh \
  deploy/enroll-dark-forge.sh \
  deploy/github/guard.sh deploy/github/ssh.sh deploy/github/ssh-session.sh deploy/github/deploy.sh \
  deploy/github/require-release.sh \
  deploy/github/install-remote.sh deploy/github/install-identity.sh deploy/github/smoke.sh \
  deploy/github/check-dashboard-bundle.sh \
  deploy/github/check-door-ssh.sh \
  deploy/github/check-installed-versions.sh \
  scripts/test-check-dashboard-bundle.sh \
  scripts/test-check-installed-versions.sh \
  scripts/test-deploy-host-tools.sh \
  scripts/test-smoke-box.sh \
  deploy/github/diagnose.sh deploy/github/diagnose-run.sh \
  scripts/test-listener-restart.sh \
  scripts/test-install-upgrade-failure-message.sh \
  scripts/test-wait-for-operations-journal-v2.sh \
  scripts/test-identity-upgrade.sh \
  scripts/test-login-token.sh \
  scripts/test-login-token-upgrade.sh \
  scripts/test-identity-slot-rehearsal.sh \
  scripts/test-installer-release.sh \
  scripts/stamp-release-version.sh scripts/stamp-release-dist.sh scripts/reuse-server-check.sh \
  scripts/wait-main-run.sh scripts/test-wait-main-run.sh \
  scripts/batch-merge.sh scripts/batch-merge-idle-gate.sh \
  scripts/test-batch-merge.sh \
  scripts/tag-path-dry-run.sh scripts/test-tag-path-dry-run.sh \
  scripts/test-tag-path-pr-stamp-order.sh \
  scripts/read-release-json-field.sh scripts/test-release-json-field.sh \
  scripts/releasable.sh scripts/test-releasable.sh \
  scripts/next-release-tag.sh scripts/test-next-release-tag.sh \
  scripts/plan-release.sh scripts/test-plan-release.sh \
  scripts/check-stamped-clients.sh scripts/test-check-stamped-clients.sh \
  scripts/check-dropped-platform.sh scripts/test-check-dropped-platform.sh \
  scripts/test-release-budget.sh \
  scripts/verify-release-attestations.sh scripts/test-verify-release-attestations.sh \
  scripts/check-feature-map.sh scripts/test-feature-map-guard.sh \
  scripts/test-operations-journal-version-sync.sh \
  scripts/test-tenant-isolation.sh \
  scripts/previous-release-tag.sh scripts/previous-release-notes-from.sh \
  scripts/published-release-tags.sh scripts/release-notes-mode.sh \
  scripts/release-notes-json.sh scripts/release-notes-markdown.sh scripts/write-release-notes.sh \
  scripts/check-embedded-release-notes.sh scripts/test-release-notes.sh \
  scripts/test-embedded-release-notes.sh \
  scripts/capture-dashboard-release-notes-proof.sh \
  scripts/fmt.sh scripts/test-fmt.sh scripts/check.sh scripts/test-check.sh scripts/check-docs.sh \
  scripts/test-ssh-multiplex.sh \
  scripts/qa/swarm.sh \
  scripts/check-release-trust.sh
