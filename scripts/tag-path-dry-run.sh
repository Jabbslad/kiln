#!/bin/sh
# Rehearse the tag path against main's latest green packages.
# The fake tag is the next patch plus -dryrun and is never pushed. The stamp,
# version-guard, draft-release, and deploy scripts accept only vX.Y.Z,
# so they receive that version with the suffix removed.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# Local agent VMs often have an invalid GH_TOKEN. gh auth status is the only
# signal; its output is discarded. CI (GITHUB_ACTIONS=true) still fails closed.
if [ "${GITHUB_ACTIONS:-}" != "true" ]; then
  if ! gh auth status >/dev/null 2>&1; then
    printf 'tag-path-dry-run: skipped: no valid local GitHub credentials, CI covers this\n'
    exit 0
  fi
fi

fail() {
  printf 'tag-path-dry-run: %s\n' "$*" >&2
  exit 1
}

green_main_run() {
  repo=$1
  row=$(
    gh run list --repo "$repo" --workflow build.yml --branch main --event push \
      --status success --limit 1 \
      --json databaseId,headSha,event,conclusion,headBranch,status \
      --jq '.[0] | select(.conclusion == "success" and .event == "push" and .headBranch == "main" and .status == "completed") | "\(.databaseId)\t\(.headSha)"'
  )
  green_main_run_id=${row%%	*}
  green_main_sha=${row#*	}
  printf '%s\n' "$green_main_sha" | grep -Eq '^[0-9a-f]{40}$' || return 1
  printf '%s\n' "$green_main_run_id" | grep -Eq '^[0-9]+$' || return 1
  return 0
}

if [ "${TAG_PATH_GREEN_MAIN_RUN_TEST:-}" = 1 ]; then
  repo=${GH_REPO:-Jabbslad/kiln}
  case $repo in
    */*) ;;
    *) fail "repository is not owner/name" ;;
  esac
  if [ -z "${GH_TOKEN:-}" ]; then
    fail 'GH_TOKEN is required'
  fi
  green_main_run "$repo" || fail 'no green main run'
  printf 'green main run %s %s\n' "$green_main_run_id" "$green_main_sha"
  exit 0
fi

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

green_main_run "$repo" || fail 'no green main run'
sha=$green_main_sha

dist=$(mktemp -d)
output=$(mktemp)
trap 'rm -rf "$dist" "$output"' EXIT

env -u REUSE_VERSION \
  GITHUB_OUTPUT="$output" \
  REUSE_EVENT=workflow_dispatch \
  REUSE_SHA_INPUT="$sha" \
  REUSE_RUN_ID="$green_main_run_id" \
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

prev_server=
for path in "$dist/kiln-server-${from}"*.tar.gz; do
  [ -f "$path" ] || continue
  prev_server=$path
  break
done
[ -n "$prev_server" ] || fail "reuse dist missing server archive for ${from}"
prev_saved=$(mktemp -d)
cp "$prev_server" "$prev_saved/"
prev_server="$prev_saved/$(basename "$prev_server")"

sh scripts/stamp-release-dist.sh "$dist" "$from" "$tag"
sh scripts/check-stamped-clients.sh "$dist" "$tag"
KILN_VERSION_CHECK_DIST="$dist" bash deploy/github/check-installed-versions.sh "$tag"
bash scripts/test-identity-slot-rehearsal.sh "$dist" "$tag"

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
grep -q 'ssh-session: dry-run control path' "$output.deploy" \
  || fail 'deploy dry-run did not set up the multiplex session'
grep -q "deploy inputs ${tag}" "$output.deploy" || fail 'deploy dry-run did not finish'
grep -q "dashboard inputs kiln-dashboard-${tag}.tar.gz" "$output.deploy" \
  || fail 'deploy dry-run did not check the dashboard archive'
grep -q 'dashboard live ' "$output.deploy" || fail 'deploy dry-run did not check the bundle'

remote_tags=$(git ls-remote --tags origin "refs/tags/${dry_tag}") || fail 'could not list tags'
[ -z "$remote_tags" ] || fail "${dry_tag} exists on the remote"

if [ "${GITHUB_EVENT_NAME:-}" = pull_request ]; then
  prev_archive=$prev_server
  VERSION=${tag#v}
  export VERSION
  cargo xtask release-version resolve --ref-type tag --ref-name "$tag"
  trust_json=$(mktemp)
  trust_bins=$(mktemp -d)
  trap 'rm -rf "$dist" "$output" "$trust_json" "$trust_bins"' EXIT
  cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client \
    --message-format=json >"$trust_json"
  cargo xtask release-version check-binary --version "$tag" -- \
    target/release/kiln \
    target/release/kiln-runtime \
    target/release/kiln-host \
    target/release/kiln-api
  image_root=$(mktemp -d)
  tar -xzf "$prev_archive" -C "$image_root" image
  pr_dist=$(mktemp -d)
  cargo xtask release server --version "$tag" --target x86_64-unknown-linux-gnu \
    --bin-dir target/release --image-dir "$image_root/image" --output "$pr_dist"
  next_archive=
  for path in "$pr_dist/kiln-server-${tag}"*.tar.gz; do
    [ -f "$path" ] || continue
    next_archive=$path
    break
  done
  [ -n "$next_archive" ] || fail 'PR server package was not assembled'
  tar -xzf "$next_archive" -C "$trust_bins" \
    bin/kiln bin/kiln-runtime bin/kiln-host bin/kiln-api
  sh scripts/check-release-trust.sh --cargo-json "$trust_json" \
    target/release/kiln \
    target/release/kiln-runtime \
    target/release/kiln-host \
    target/release/kiln-api \
    "$trust_bins/bin/kiln" \
    "$trust_bins/bin/kiln-runtime" \
    "$trust_bins/bin/kiln-host" \
    "$trust_bins/bin/kiln-api"
  rm -rf "$trust_json" "$trust_bins"
  KILN_SERVER_UPGRADE_PREV_TAR=$prev_archive \
    KILN_SERVER_UPGRADE_TO_TAR=$next_archive \
    bash scripts/test-identity-upgrade.sh
  proved_dist=$root/dist-proved-package-server
  mkdir -p "$proved_dist"
  git checkout -- Cargo.toml Cargo.lock
  # From-tag bytes used to come from server-check. Building them here lets
  # tag-path run beside that job. Proof adoption still requires a green run.
  reuse_bin_dir=${TAG_PATH_RELEASE_BIN_DIR:-}
  if [ -z "$reuse_bin_dir" ]; then
    VERSION=$from
    export VERSION
    cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client
    cargo build --release --locked -p kiln-identity
    reuse_bin_dir=target/release
  fi
  image_root=$(mktemp -d)
  tar -xzf "$prev_archive" -C "$image_root" image
  VERSION=$from
  export VERSION
  SKIP_RELEASE_BUILD=1 \
    RELEASE_BIN_DIR="$reuse_bin_dir" \
    KILN_PACKAGE_IMAGE_DIR="$image_root/image" \
    PACKAGE_OUTPUT=$proved_dist sh scripts/package-server-release.sh
  rm -rf "$image_root"
  stamp_rehearsal=$(mktemp -d)
  cp "$proved_dist"/*.tar.gz "$proved_dist"/*.sha256 "$stamp_rehearsal/"
  sh scripts/stamp-release-dist.sh "$stamp_rehearsal" "$from" "$tag"
  found=
  for path in "$stamp_rehearsal/kiln-server-${tag}"*.tar.gz; do
    [ -f "$path" ] || continue
    found=1
    break
  done
  [ -n "$found" ] || fail 'main-push stamp rehearsal did not rename proved server archive'
  rm -rf "$stamp_rehearsal"
fi

printf 'tag path dry-run ok %s\n' "$dry_tag"
