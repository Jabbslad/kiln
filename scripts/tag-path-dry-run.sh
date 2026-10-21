#!/bin/sh
# Rehearse the tag path against main's latest green packages.
# The fake tag is the next patch plus -dryrun and is never pushed. The stamp,
# version-guard, draft-release, and deploy scripts accept only vX.Y.Z,
# so they receive that version with the suffix removed.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'tag-path-dry-run: %s\n' "$*" >&2
  exit 1
}

sh scripts/test-releasable.sh
sh scripts/test-next-release-tag.sh
sh scripts/test-plan-release.sh
sh scripts/test-check-stamped-clients.sh
sh scripts/test-release-budget.sh

repo=${GH_REPO:-Jabbslad/kiln}
case $repo in
  */*) ;;
  *) fail "repository is not owner/name" ;;
esac
if [ -z "${GH_TOKEN:-}" ]; then
  fail 'GH_TOKEN is required'
fi

sha=$(
  gh run list --repo "$repo" --workflow build.yml --branch main --event push \
    --status success --limit 1 \
    --json headSha,event,conclusion,headBranch,status \
    --jq '.[0] | select(.conclusion == "success" and .event == "push" and .headBranch == "main" and .status == "completed") | .headSha'
)
printf '%s\n' "$sha" | grep -Eq '^[0-9a-f]{40}$' || fail 'no green main run'

dist=$(mktemp -d)
output=$(mktemp)
trap 'rm -rf "$dist" "$output"' EXIT

env -u REUSE_VERSION \
  GITHUB_OUTPUT="$output" \
  REUSE_EVENT=workflow_dispatch \
  REUSE_SHA_INPUT="$sha" \
  GH_REPO="$repo" \
  GH_TOKEN="$GH_TOKEN" \
  cargo xtask reuse-release fetch --dist "$dist"

from=$(awk -F= '$1 == "version" { print $2 }' "$output")
mode=$(awk -F= '$1 == "mode" { print $2 }' "$output")
[ "$mode" = reuse ] || fail "reuse mode is ${mode:-empty}"
printf '%s\n' "$from" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "from-tag ${from}"

tags=$(git ls-remote --tags origin 'refs/tags/v*') || fail 'could not list tags'
tag=$(printf '%s\n' "$tags" | sh scripts/next-release-tag.sh)
dry_tag=${tag}-dryrun
printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "next tag ${tag}"
printf '%s\n' "$dry_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+-dryrun$' || fail "fake tag ${dry_tag}"
printf 'dry-run tag %s sha %s\n' "$tag" "$sha"
printf 'dry-run release %s\n' "$tag"

sh scripts/stamp-release-dist.sh "$dist" "$from" "$tag"
sh scripts/check-stamped-clients.sh "$dist" "$tag"
KILN_VERSION_CHECK_DIST="$dist" bash deploy/github/check-installed-versions.sh "$tag"

count=0
for path in "$dist"/*.tar.gz; do
  [ -f "$path" ] || continue
  name=$(basename "$path")
  case $name in
    *"$tag"*) ;;
    *) fail "attest subject ${name} is not ${tag}" ;;
  esac
  case $name in
    *dryrun*) fail "attest subject ${name} uses the dry-run suffix" ;;
  esac
  printf 'attest input %s\n' "$name"
  count=$((count + 1))
done
[ "$count" -gt 0 ] || fail 'no attest subjects'
printf 'attest source-ref refs/heads/main\n'

cargo xtask reuse-release finish --version "$tag" --dist "$dist"
(cd "$dist" && sha256sum -c ./*.sha256)
if [ ! -f "$dist/install.sh" ] || [ ! -f "$dist/install.sh.sha256" ]; then
  fail 'draft-release did not assemble install.sh'
fi

KILN_DEPLOY_DRY_RUN=1 \
  KILN_RELEASE_TAG="$tag" \
  KILN_DEPLOY_DIST="$dist" \
  bash deploy/github/deploy.sh >"$output.deploy" 2>&1
grep -q "deploy inputs ${tag}" "$output.deploy" || fail 'deploy dry-run did not finish'
grep -q "dashboard inputs kiln-dashboard-${tag}.tar.gz" "$output.deploy" \
  || fail 'deploy dry-run did not check the dashboard archive'
grep -q 'dashboard live ' "$output.deploy" || fail 'deploy dry-run did not check the bundle'

remote_tags=$(git ls-remote --tags origin "refs/tags/${dry_tag}") || fail 'could not list tags'
[ -z "$remote_tags" ] || fail "${dry_tag} exists on the remote"
printf 'tag path dry-run ok %s\n' "$dry_tag"
