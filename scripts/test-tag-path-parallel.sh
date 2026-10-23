#!/bin/sh
# Tag-path runs beside server-check. The release-bin upload is gone.
# A failed server-check still cannot be adopted: proofs stay success-only.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-tag-path-parallel: %s\n' "$*" >&2
  exit 1
}

workflow=$root/.github/workflows/build.yml
tag_script=$root/scripts/tag-path-dry-run.sh
proofs=$root/xtask/src/merge_queue.rs

job_body() {
  name=$1
  awk -v name="$name" '
    $0 == "  " name ":" { flag = 1; next }
    flag && /^  [a-z0-9][a-z0-9-]*:/ { exit }
    flag { print }
  ' "$workflow"
}

server=$(job_body server-check)
tag_job=$(job_body tag-path-dry-run)
required=$(job_body required)
cancel=$(job_body cancel-on-required-failure)
[ -n "$server" ] || fail 'missing server-check'
[ -n "$tag_job" ] || fail 'missing tag-path-dry-run'
[ -n "$required" ] || fail 'missing required'

if printf '%s\n' "$server" | grep -q 'server-check-release-bins'; then
  fail 'server-check must not upload release bins'
fi
if printf '%s\n' "$server" | grep -q 'cargo build --release'; then
  fail 'server-check must not build release binaries'
fi
printf '%s\n' "$server" | grep -q 'scripts/server-check-run-test-and-lint.sh' \
  || fail 'server-check must still run the test-and-lint suite'

printf '%s\n' "$tag_job" | grep -Fqx '    needs: [changes]' \
  || fail 'tag-path-dry-run must need only changes'
if printf '%s\n' "$tag_job" | grep -q 'server-check'; then
  fail 'tag-path-dry-run must not wait on server-check'
fi
if printf '%s\n' "$tag_job" | grep -q 'actions/download-artifact'; then
  fail 'tag-path-dry-run must not download release bins'
fi
printf '%s\n' "$tag_job" | grep -q 'Swatinem/rust-cache@' \
  || fail 'tag-path-dry-run must keep rust-cache'
printf '%s\n' "$tag_job" | grep -q 'pull_request' \
  || fail 'tag-path-dry-run must still run on pull requests'
printf '%s\n' "$tag_job" | grep -q 'name: package-server' \
  || fail 'tag-path-dry-run must still upload package-server'

printf '%s\n' "$required" | grep -Fqx '    needs: [changes, docs-check, feature-map-guard, server-check, client, dashboard]' \
  || fail 'required needs must stay unchanged'
printf '%s\n' "$required" | grep -q 'cargo xtask required-gate' \
  || fail 'required must still run required-gate'
printf '%s\n' "$required" | grep -q 'actions: read' \
  || fail 'required must be able to read job timings'
budget_line=$(printf '%s\n' "$required" | grep -n 'scripts/ci-budget.sh' | awk -F: 'NR == 1 { print $1 }')
gate_line=$(printf '%s\n' "$required" | grep -n 'cargo xtask required-gate' | awk -F: 'NR == 1 { print $1 }')
[ -n "$budget_line" ] || fail 'required must run scripts/ci-budget.sh'
[ -n "$gate_line" ] || fail 'required-gate line missing'
[ "$budget_line" -lt "$gate_line" ] || fail 'ci-budget must run before required-gate'
if printf '%s\n' "$required" | grep -q '^    name:'; then
  fail 'required block must not use a 4-space name key'
fi

printf '%s\n' "$cancel" | grep -q 'server-check' \
  || fail 'cancel-on-required-failure must still observe server-check'
for job in changes docs-check feature-map-guard server-check client dashboard required tag-path-dry-run cancel-on-required-failure; do
  grep -q "^  ${job}:" "$workflow" || fail "missing job $job"
done

if grep -q 'TAG_PATH_RELEASE_BIN_DIR is required' "$tag_script"; then
  fail 'tag-path must build from-tag binaries when the reuse dir is empty'
fi
empty=$(
  awk '
    /reuse_bin_dir=\$\{TAG_PATH_RELEASE_BIN_DIR:-\}/ { flag = 1 }
    flag { print }
    flag && /^  fi$/ { exit }
  ' "$tag_script"
)
[ -n "$empty" ] || fail 'missing empty reuse-dir build'
# shellcheck disable=SC2016 # literal shell text from tag-path-dry-run.sh
printf '%s\n' "$empty" | grep -Fq 'VERSION=$from' \
  || fail 'empty reuse dir must build at the from-tag'
printf '%s\n' "$empty" | grep -q 'cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client' \
  || fail 'empty reuse dir must build the server bins'
proved=$(
  awk '
    /reuse_bin_dir=\$\{TAG_PATH_RELEASE_BIN_DIR:-\}/ { flag = 1 }
    flag { print }
    flag && /package-server-release.sh/ { exit }
  ' "$tag_script"
)
printf '%s\n' "$proved" | grep -q 'SKIP_RELEASE_BUILD=1' \
  || fail 'proved package must still skip a second release build'

grep -q 'status=success' "$proofs" \
  || fail 'merge-queue proofs must list only successful runs'
grep -q '!= Some("success")' "$proofs" \
  || fail 'merge-queue proofs must reject a non-success conclusion'

for site in scripts/server-check-test-and-lint.sh scripts/check-docs.sh .github/workflows/build.yml; do
  grep -q 'sh scripts/test-tag-path-parallel.sh' "$site" \
    || fail "$site must run test-tag-path-parallel.sh"
  grep -q 'sh scripts/test-ci-budget.sh' "$site" \
    || fail "$site must run test-ci-budget.sh"
done
grep -q 'scripts/ci-budget.sh' scripts/server-check-test-and-lint.sh \
  || fail 'server-check shellcheck list must include ci-budget.sh'

printf 'tag-path parallel ok\n'
