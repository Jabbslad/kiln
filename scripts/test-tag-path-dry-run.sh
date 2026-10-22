#!/bin/sh
# The bytes below are release.json from main run 39568231107, the packages
# v0.4.18 tried to stamp. A spaced "version": "v0.4.17" is not what they contain.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-tag-path-dry-run: %s\n' "$*" >&2
  exit 1
}

fixture=$root/scripts/fixtures/main-39568231107
identity_json=$fixture/identity-release.json
server_json=$fixture/server-release.json
[ "$(sha256sum "$identity_json" | awk 'NR == 1 { print $1 }')" = 6f765aa2a525fcd93e449e67a6d78083d3ceab70c3c803b63490ab04561a2bc9 ] \
  || fail 'identity release.json is not the main package'
[ "$(sha256sum "$server_json" | awk 'NR == 1 { print $1 }')" = 9203384fcffb4f903aabcf60d3c36be3c9bc50451a68aaf58797e11f73047c49 ] \
  || fail 'server release.json is not the main package'

if grep -n -E 'gh release|git push|git tag|ssh ' "$root/scripts/tag-path-dry-run.sh" >/dev/null; then
  fail 'dry-run script publishes, tags, or uses ssh'
fi
grep -q 'scripts/check-release-trust.sh' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'dry-run must guard release binaries'
grep -q 'scripts/check-release-trust.sh' "$root/.github/workflows/build.yml" \
  || fail 'release build must guard release binaries'

grep -q 'resolve_reuse_main_run' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must resolve a green main run for the merge base'
grep -q 'tag_path_merge_base' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must compute the PR merge base'
grep -q 'tag-path-fallback-dist.sh' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must fall back to a local dist build'
grep -q 'databaseId' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must pin the green main workflow run id'
grep -q 'REUSE_RUN_ID' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must pass REUSE_RUN_ID into reuse-release fetch'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

slot() {
  mkdir -p "$(dirname "$1")"
  {
    printf 'KILNRELSLOT='
    dd if=/dev/zero bs=32 count=1 status=none
  } >"$1"
  chmod 755 "$1"
}

pack_archive() {
  name=$1
  dir=$2
  shift 2
  archive=$work/dist/$name
  tar -C "$dir" -czf "$archive" "$@"
  digest=$(sha256sum "$archive" | awk 'NR == 1 { print $1 }')
  printf '%s  %s\n' "$digest" "$name" >"${archive}.sha256"
}

mkdir -p "$work/dist" "$work/identity/bin" "$work/server/bin"
cp "$identity_json" "$work/identity/release.json"
cp "$server_json" "$work/server/release.json"
slot "$work/identity/bin/kiln-identity"
slot "$work/server/bin/kiln"
printf 'install\n' >"$work/server/install.sh"
pack_archive kiln-identity-v0.4.17-x86_64-unknown-linux-gnu.tar.gz "$work/identity" \
  release.json bin/kiln-identity
pack_archive kiln-server-v0.4.17-x86_64-unknown-linux-gnu.tar.gz "$work/server" \
  release.json bin/kiln install.sh

if ! sh "$root/scripts/stamp-release-dist.sh" "$work/dist" v0.4.17 v0.4.18 >"$work/out" 2>"$work/err"; then
  cat "$work/err" >&2
  fail 'stamp did not accept main release.json'
fi

for name in \
  kiln-identity-v0.4.17-x86_64-unknown-linux-gnu.tar.gz \
  kiln-server-v0.4.17-x86_64-unknown-linux-gnu.tar.gz
do
  [ ! -e "$work/dist/$name" ] || fail "old archive ${name} remains"
done
(cd "$work/dist" && sha256sum -c ./*.sha256)

want_identity='{"target":"x86_64-unknown-linux-gnu","version":"v0.4.18"}'
want_server='{"min_upgrade_from":"0.3.0","target":"x86_64-unknown-linux-gnu","version":"v0.4.18"}'
tar -xOzf "$work/dist/kiln-identity-v0.4.18-x86_64-unknown-linux-gnu.tar.gz" release.json >"$work/got-identity"
tar -xOzf "$work/dist/kiln-server-v0.4.18-x86_64-unknown-linux-gnu.tar.gz" release.json >"$work/got-server"
printf '%s\n' "$want_identity" >"$work/want-identity"
printf '%s\n' "$want_server" >"$work/want-server"
cmp -s "$work/got-identity" "$work/want-identity" || fail 'identity release.json was not stamped'
cmp -s "$work/got-server" "$work/want-server" || fail 'server release.json was not stamped'

mkdir -p "$work/zipdist"
printf 'x\n' >"$work/zipdist/kiln-v0.4.17-x86_64-pc-windows-msvc.zip"
if sh "$root/scripts/stamp-release-dist.sh" "$work/zipdist" v0.4.17 v0.4.18 \
  >"$work/zip-out" 2>"$work/zip-err"; then
  fail 'stamp accepted a windows zip'
fi
grep -q 'unexpected dist file kiln-v0.4.17-x86_64-pc-windows-msvc.zip' "$work/zip-err" \
  || fail "stamp did not reject the windows zip: $(cat "$work/zip-err")"

mkdir -p "$work/bin" "$work/deploy/bin"
cat >"$work/bin/ssh" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> '$work/ssh.log'
exit 99
EOF
cp "$work/bin/ssh" "$work/bin/curl"
chmod 755 "$work/bin/ssh" "$work/bin/curl"
write_version_bin() {
  path=$1
  name=$2
  version=$3
  cat >"$path" <<EOF
#!/bin/sh
if [ "\${1:-}" = --version ]; then
  printf '%s %s\n' '$name' '$version'
  exit 0
fi
exit 2
EOF
  chmod 755 "$path"
}
write_version_bin "$work/deploy/bin/kiln-identity" kiln-identity 0.4.18
write_version_bin "$work/deploy/bin/kiln" kiln 0.4.18
write_version_bin "$work/deploy/bin/kiln-runtime" kiln-runtime 0.4.18
write_version_bin "$work/deploy/bin/kiln-host" kiln-server 0.4.18
write_version_bin "$work/deploy/bin/kiln-api" kiln-server 0.4.18
printf '%s\n' "$want_identity" >"$work/deploy/release.json"
printf 'install\n' >"$work/deploy/install.sh"
mkdir -p "$work/deploy-dist"
tar -C "$work/deploy" -czf "$work/deploy-dist/kiln-identity-v0.4.18-x86_64-unknown-linux-gnu.tar.gz" release.json bin/kiln-identity
tar -C "$work/deploy" -czf "$work/deploy-dist/kiln-server-v0.4.18-x86_64-unknown-linux-gnu.tar.gz" \
  install.sh bin/kiln bin/kiln-runtime bin/kiln-host bin/kiln-api
mkdir -p "$work/dashboard/assets"
printf '<script type="module" crossorigin src="/assets/index-dryrun.js"></script>\n' \
  >"$work/dashboard/index.html"
printf 'js\n' >"$work/dashboard/assets/index-dryrun.js"
tar -C "$work/dashboard" -czf "$work/deploy-dist/kiln-dashboard-v0.4.18.tar.gz" \
  index.html assets/index-dryrun.js

for archive in "$work/deploy-dist"/*.tar.gz; do
  base=$(basename "$archive")
  digest=$(sha256sum "$archive" | awk 'NR == 1 { print $1 }')
  printf '%s  %s\n' "$digest" "$base" >"${archive}.sha256"
done
: >"$work/ssh.log"
GITHUB_EVENT_NAME=pull_request \
  GITHUB_REPOSITORY=Jabbslad/kiln \
  GITHUB_REF=refs/pull/1/merge \
  KILN_DEPLOY_DRY_RUN=1 \
  KILN_RELEASE_TAG=v0.4.18 \
  KILN_DEPLOY_DIST="$work/deploy-dist" \
  KILN_SSH_SH="$work/bin/ssh" \
  PATH="$work/bin:$PATH" \
  bash "$root/deploy/github/deploy.sh" >"$work/deploy-out" 2>"$work/deploy-err" \
  || {
    cat "$work/deploy-err" >&2
    fail 'deploy dry-run rejected its inputs'
  }
grep -q 'identity inputs 0.4.18' "$work/deploy-out" || fail 'identity inputs were not checked'
grep -q 'server inputs kiln-server-v0.4.18-x86_64-unknown-linux-gnu.tar.gz' "$work/deploy-out" \
  || fail 'server inputs were not checked'
grep -q 'ssh-session: dry-run control path' "$work/deploy-out" \
  || fail 'deploy dry-run did not set up the multiplex session'
grep -q 'deploy inputs v0.4.18' "$work/deploy-out" || fail 'deploy inputs were not reported'
grep -q 'dashboard inputs kiln-dashboard-v0.4.18.tar.gz' "$work/deploy-out" \
  || fail 'dashboard inputs were not checked'
grep -q 'dashboard live index-dryrun.js' "$work/deploy-out" \
  || fail 'dashboard bundle check did not run'
grep -q 'packaged versions v0.4.18' "$work/deploy-out" \
  || fail 'deploy dry-run did not check packaged binary versions'
[ ! -s "$work/ssh.log" ] || fail 'dry-run called ssh or curl'

old_green=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
new_main=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
failed_main=dddddddddddddddddddddddddddddddddddddddd
orphan_green=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
merge_base=$old_green
green_run_id=8888
real_gh=$(command -v gh)

write_gh_shim() {
  run_list_tsv=$1
  mkdir -p "$work/bin"
  cat >"$work/bin/gh" <<EOF
#!/bin/sh
if [ "\$1" = run ] && [ "\$2" = list ]; then
  case "\$*" in
    *--commit*)
      printf '[]\n'
      exit 0
      ;;
    *--branch*main*)
      cat <<'RUNLIST'
$run_list_tsv
RUNLIST
      exit 0
      ;;
  esac
fi
if [ "\$1" = api ]; then
  case "\$*" in
    *actions/runs/*/artifacts*)
      cat <<'ARTS'
{"total_count":5,"artifacts":[
{"name":"package-aarch64-apple-darwin","expired":false,"expires_at":"2099-01-01T00:00:00Z"},
{"name":"package-dashboard","expired":false,"expires_at":"2099-01-01T00:00:00Z"},
{"name":"package-server","expired":false,"expires_at":"2099-01-01T00:00:00Z"},
{"name":"package-x86_64-apple-darwin","expired":false,"expires_at":"2099-01-01T00:00:00Z"},
{"name":"package-x86_64-unknown-linux-gnu","expired":false,"expires_at":"2099-01-01T00:00:00Z"}
]}
ARTS
      exit 0
      ;;
    *actions/runs/$green_run_id*--jq*path*)
      printf '.github/workflows/build.yml\n'
      exit 0
      ;;
    *actions/runs/$green_run_id*)
      printf '{"databaseId":$green_run_id,"headSha":"$old_green","headBranch":"main","event":"push","conclusion":"success","status":"completed","path":".github/workflows/build.yml"}\n'
      exit 0
      ;;
    *contents/Cargo.toml*)
      cat <<'CARGO'
[workspace]
members = ["xtask"]

[workspace.package]
version = "0.4.17"
CARGO
      exit 0
      ;;
  esac
fi
if [ "\$1" = run ] && [ "\$2" = download ]; then
  mkdir -p "\${@: -1}"
  exit 0
fi
exec "$real_gh" "\$@"
EOF
  chmod +x "$work/bin/gh"
}

resolve_reuse() {
  TAG_PATH_REUSE_RESOLVE_TEST=1 \
    TAG_PATH_MERGE_BASE="$merge_base" \
    TAG_PATH_ANCESTOR_SHAS="${TAG_PATH_ANCESTOR_SHAS:-$old_green}" \
    GH_REPO=Jabbslad/kiln \
    GH_TOKEN=test-token \
    PATH="$work/bin:$PATH" \
    sh "$root/scripts/tag-path-dry-run.sh" >"$work/resolve-out" 2>"$work/resolve-err"
}

in_progress_runs='9999	'"$new_main"'		in_progress	push	main
'"$green_run_id"'	'"$old_green"'	success	completed	push	main'
write_gh_shim "$in_progress_runs"
if ! resolve_reuse; then
  cat "$work/resolve-err" >&2
  fail 'reuse resolve must succeed while a newer main build is in progress'
fi
grep -q "reuse main run $green_run_id $old_green" "$work/resolve-out" \
  || fail 'reuse resolve must pin the last green ancestor, not the in-progress main build'

failed_runs='7777	'"$failed_main"'	failure	completed	push	main
'"$green_run_id"'	'"$old_green"'	success	completed	push	main'
write_gh_shim "$failed_runs"
if ! resolve_reuse; then
  cat "$work/resolve-err" >&2
  fail 'reuse resolve must succeed while the newest main build failed'
fi
grep -q "reuse main run $green_run_id $old_green" "$work/resolve-out" \
  || fail 'reuse resolve must pin the last green ancestor after a failed main build'

no_ancestor_runs=$green_run_id'	'"$orphan_green"'	success	completed	push	main'
write_gh_shim "$no_ancestor_runs"
TAG_PATH_ANCESTOR_SHAS=$old_green
if ! resolve_reuse; then
  cat "$work/resolve-err" >&2
  fail 'reuse resolve must not fail when merge base has no green ancestor'
fi
grep -q 'reuse source fallback' "$work/resolve-out" \
  || fail 'reuse resolve must report local fallback without a green ancestor'

if TAG_PATH_GENUINE_FAILURE_TEST=1 \
  TAG_PATH_TEST_FROM=v0.4.17 \
  sh "$root/scripts/tag-path-dry-run.sh" >"$work/genuine-out" 2>"$work/genuine-err"; then
  cat "$work/genuine-err" >&2
  fail 'tag-path dry-run must fail on a missing reuse server archive'
fi
grep -q 'reuse dist missing server archive for v0.4.17' "$work/genuine-err" \
  || fail 'tag-path dry-run must report a genuine reuse dist break'

TAG_PATH_ANCESTOR_SHAS=$old_green
write_gh_shim "$in_progress_runs"

align_dist=$work/reuse-dist
align_out=$work/reuse-output
mkdir -p "$align_dist"
env -u REUSE_VERSION \
  GITHUB_OUTPUT="$align_out" \
  REUSE_EVENT=workflow_dispatch \
  REUSE_SHA_INPUT="$old_green" \
  GH_REPO=Jabbslad/kiln \
  GH_TOKEN=test-token \
  PATH="$work/bin:$PATH" \
  cargo xtask reuse-release fetch --dist "$align_dist" >"$work/unpinned.stdout" 2>"$work/unpinned.stderr" \
  || fail 'unpinned reuse fetch must exit cleanly into fallback'
grep -q 'no successful main push of this workflow for that commit' "$work/unpinned.stderr" \
  || fail 'unpinned reuse fetch must refuse with no successful main push during commit-list race'
grep -q 'mode=fallback' "$align_out" \
  || fail 'unpinned reuse fetch must record fallback mode'

if ! env -u REUSE_VERSION \
  GITHUB_OUTPUT="$align_out" \
  REUSE_EVENT=workflow_dispatch \
  REUSE_SHA_INPUT="$old_green" \
  REUSE_RUN_ID="$green_run_id" \
  GH_REPO=Jabbslad/kiln \
  GH_TOKEN=test-token \
  PATH="$work/bin:$PATH" \
  cargo xtask reuse-release fetch --dist "$align_dist" >"$work/pinned.stdout" 2>"$work/pinned.stderr"; then
  :
fi
if grep -q 'no successful main push of this workflow for that commit' "$work/pinned.stderr"; then
  fail 'pinned reuse fetch must not refuse when REUSE_RUN_ID matches the green main run'
fi

if ! cargo xtask test-reuse-release >"$work/reuse-test.log" 2>&1; then
  cat "$work/reuse-test.log" >&2
  fail 'reuse-release unit tests failed'
fi
grep -q 'test_pinned_run_resolves_when_commit_list_is_empty' "$work/reuse-test.log" \
  || fail 'reuse-release must cover pinned run id when commit list is empty'

printf 'tag-path dry-run tests ok\n'
