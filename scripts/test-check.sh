#!/bin/sh
# Guard: scripts/check.sh default path stays aligned with required fast checks.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check: %s\n' "$*" >&2
  exit 1
}

check=$root/scripts/check.sh
body=$(cat "$check")

printf '%s\n' "$body" | grep -q 'scripts/fmt.sh --check' \
  || fail 'check.sh must run fmt in check mode'
printf '%s\n' "$body" | grep -q 'cargo clippy --workspace' \
  || fail 'check.sh must run clippy'
printf '%s\n' "$body" | grep -q 'run-workspace-nextest.sh' \
  || fail 'check.sh must run workspace tests through run-workspace-nextest.sh'
printf '%s\n' "$body" | grep -q 'check-feature-map.sh' \
  || fail 'check.sh must run the feature-map guard'
printf '%s\n' "$body" | grep -q '\-\-docs-check' \
  || fail 'check.sh must expose --docs-check'
printf '%s\n' "$body" | grep -q '\-\-release' \
  || fail 'check.sh must expose --release'
printf '%s\n' "$body" | grep -q 'scripts/check-docs.sh' \
  || fail 'check.sh must wire --docs-check to check-docs.sh'
printf '%s\n' "$body" | grep -q 'scripts/tag-path-dry-run.sh' \
  || fail 'check.sh must wire --release to tag-path-dry-run'
printf '%s\n' "$body" | grep -q 'shellcheck not installed, skipped' \
  || fail 'check.sh must skip shellcheck when missing'

grep -q 'sh scripts/test-check.sh' .github/workflows/build.yml \
  || fail 'build.yml must run test-check.sh'
grep -q 'sh scripts/test-server-check-nextest.sh' .github/workflows/build.yml \
  || fail 'build.yml must run test-server-check-nextest.sh'
grep -q 'sh scripts/test-pre-push-hook.sh' .github/workflows/build.yml \
  || fail 'build.yml must run test-pre-push-hook.sh'
grep -q 'scripts/check.sh' .githooks/pre-push \
  || fail 'pre-push hook must run scripts/check.sh'
grep -q 'prepush-extra-check-paths.sh' .githooks/pre-push \
  || fail 'pre-push hook must use prepush-extra-check-paths.sh'

docs=$root/scripts/check-docs.sh
grep -q 'sh scripts/test-check-pr-proof.sh' "$docs" \
  || fail 'check-docs.sh must run test-check-pr-proof.sh (docs-check parity)'
grep -q 'sh scripts/test-check.sh' "$docs" \
  || fail 'check-docs.sh must run test-check.sh (docs-check parity)'
grep -q 'sh scripts/test-tag-path-pr-stamp-order.sh' "$docs" \
  || fail 'check-docs.sh must run test-tag-path-pr-stamp-order.sh (docs-check parity)'
grep -q 'sh scripts/test-deploy-host-tools.sh' "$docs" \
  || fail 'check-docs.sh must run test-deploy-host-tools.sh (docs-check parity)'
grep -q 'sh scripts/test-release-json-field.sh' "$docs" \
  || fail 'check-docs.sh must run test-release-json-field.sh (docs-check parity)'
grep -q 'sh scripts/test-release-notes.sh' "$docs" \
  || fail 'check-docs.sh must run test-release-notes.sh (docs-check parity)'
grep -q 'bash scripts/test-identity-upgrade.sh' "$docs" \
  || fail 'check-docs.sh must run test-identity-upgrade.sh (docs-check parity)'
grep -q 'sh scripts/test-verify-release-attestations.sh' "$docs" \
  || fail 'check-docs.sh must run test-verify-release-attestations.sh (docs-check parity)'

printf '%s\n' "$body" | awk '/^run_server_check\(\)/,/^}/' | grep -q 'run-workspace-nextest.sh &&' \
  || fail 'run_server_check must chain steps with && so failures propagate under run_check'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
real_cargo=$(command -v cargo)
cat >"$work/cargo" <<EOF
#!/bin/sh
if [ "\$1" = clippy ]; then
  exit 1
fi
exec "$real_cargo" "\$@"
EOF
chmod +x "$work/cargo"

if out=$(PATH="$work:$PATH" sh "$check" 2>&1); then
  printf '%s\n' "$out" >&2
  fail 'check.sh must exit non-zero when server-check clippy fails'
fi
printf '%s\n' "$out" | grep -q 'FAILED: server-check' \
  || fail 'check.sh must print FAILED: server-check when clippy fails'

nextest_script=$root/scripts/run-workspace-nextest.sh
nextest_saved=$work/run-workspace-nextest.saved
cp "$nextest_script" "$nextest_saved"
tag_script=$root/scripts/tag-path-dry-run.sh
tag_saved=$work/tag-path-dry-run.saved
cp "$tag_script" "$tag_saved"
restore_scripts() {
  cp "$nextest_saved" "$nextest_script"
  cp "$tag_saved" "$tag_script"
}
trap 'restore_scripts; rm -rf "$work"' EXIT
printf '%s\n' '#!/bin/sh' 'exit 0' >"$nextest_script"
chmod +x "$nextest_script"
printf '%s\n' '#!/bin/sh' 'exit 1' >"$tag_script"
chmod +x "$tag_script"

if out=$(GH_TOKEN=test-token sh "$check" --release 2>&1); then
  printf '%s\n' "$out" >&2
  fail 'check.sh --release must exit non-zero when tag-path-dry-run fails'
fi
printf '%s\n' "$out" | grep -q 'FAILED: tag-path-dry-run' \
  || fail 'check.sh --release must print FAILED: tag-path-dry-run when dry-run fails'
restore_scripts
trap 'rm -rf "$work"' EXIT

printf 'check wiring tests ok\n'
