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
lint=$root/scripts/server-check-test-and-lint.sh
grep -h 'scripts/check-release-trust.sh' "$root/.github/workflows/build.yml" "$lint" >/dev/null \
  || fail 'release build must guard release binaries'

grep -q 'green_main_run' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must resolve the latest green main run'
grep -q 'databaseId' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must pin the green main workflow run id'
grep -q 'REUSE_RUN_ID' "$root/scripts/tag-path-dry-run.sh" \
  || fail 'tag-path-dry-run must pass REUSE_RUN_ID into reuse-release fetch'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Off CI, invalid gh auth must skip. CI must still fail closed. The stub never
# reaches the network; only its exit status is consulted.
auth_bin=$work/auth-bin
mkdir -p "$auth_bin"
cat >"$auth_bin/gh" <<'EOF'
#!/bin/sh
if [ "${1:-}" = auth ] && [ "${2:-}" = status ]; then
  exit 1
fi
printf '%s\n' 'HTTP 401: Bad credentials' >&2
exit 1
EOF
chmod +x "$auth_bin/gh"
skip_line='tag-path-dry-run: skipped: no valid local GitHub credentials, CI covers this'

if ! env -u GITHUB_ACTIONS \
  TAG_PATH_GREEN_MAIN_RUN_TEST=1 \
  GH_REPO=Jabbslad/kiln \
  GH_TOKEN=invalid \
  PATH="$auth_bin:$PATH" \
  sh "$root/scripts/tag-path-dry-run.sh" >"$work/auth-off.out" 2>"$work/auth-off.err"; then
  cat "$work/auth-off.out" "$work/auth-off.err" >&2
  fail 'off CI, invalid gh auth must skip and exit 0'
fi
cat "$work/auth-off.out" "$work/auth-off.err" >"$work/auth-off.combined"
printf '%s\n' "$skip_line" >"$work/auth-off.want"
cmp -s "$work/auth-off.combined" "$work/auth-off.want" \
  || fail "off CI must print exactly: ${skip_line}"

if env GITHUB_ACTIONS=true \
  TAG_PATH_GREEN_MAIN_RUN_TEST=1 \
  GH_REPO=Jabbslad/kiln \
  GH_TOKEN=invalid \
  PATH="$auth_bin:$PATH" \
  sh "$root/scripts/tag-path-dry-run.sh" >"$work/auth-ci.out" 2>"$work/auth-ci.err"; then
  cat "$work/auth-ci.out" "$work/auth-ci.err" >&2
  fail 'CI must fail when gh auth is invalid'
fi
if grep -F -x "$skip_line" "$work/auth-ci.out" "$work/auth-ci.err" >/dev/null; then
  fail 'CI must not skip tag-path-dry-run when gh auth is invalid'
fi

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
green_run_id=8888
real_gh=$(command -v gh)
mkdir -p "$work/bin"
cat >"$work/bin/gh" <<EOF
#!/bin/sh
if [ "\$1" = auth ] && [ "\$2" = status ]; then
  exit 0
fi
if [ "\$1" = run ] && [ "\$2" = list ]; then
  case "\$*" in
    *--commit*)
      printf '[]\n'
      exit 0
      ;;
    *--status*success*)
      printf '%s\t%s\n' "$green_run_id" "$old_green"
      exit 0
      ;;
    *--branch*main*)
      printf '[{"databaseId":9999,"headSha":"$new_main","event":"push","conclusion":"","headBranch":"main","status":"in_progress"}]\n'
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

if ! TAG_PATH_GREEN_MAIN_RUN_TEST=1 \
  GH_REPO=Jabbslad/kiln \
  GH_TOKEN=test-token \
  PATH="$work/bin:$PATH" \
  sh "$root/scripts/tag-path-dry-run.sh" >"$work/green-out" 2>"$work/green-err"; then
  cat "$work/green-err" >&2
  fail 'green main run lookup must succeed while a newer main build is in progress'
fi
grep -q "green main run $green_run_id $old_green" "$work/green-out" \
  || fail 'green main run lookup must pin the last green run, not the in-progress main build'

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
