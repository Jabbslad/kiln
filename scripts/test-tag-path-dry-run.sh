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
cat >"$work/deploy/bin/kiln-identity" <<'EOF'
#!/bin/sh
printf '%s\n' 'kiln-identity 0.4.18'
EOF
chmod 755 "$work/deploy/bin/kiln-identity"
printf '%s\n' "$want_identity" >"$work/deploy/release.json"
printf 'install\n' >"$work/deploy/install.sh"
mkdir -p "$work/deploy-dist"
tar -C "$work/deploy" -czf "$work/deploy-dist/kiln-identity-v0.4.18-x86_64-unknown-linux-gnu.tar.gz" release.json bin/kiln-identity
tar -C "$work/deploy" -czf "$work/deploy-dist/kiln-server-v0.4.18-x86_64-unknown-linux-gnu.tar.gz" install.sh
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
grep -q 'deploy inputs v0.4.18' "$work/deploy-out" || fail 'deploy inputs were not reported'
grep -q 'dashboard inputs kiln-dashboard-v0.4.18.tar.gz' "$work/deploy-out" \
  || fail 'dashboard inputs were not checked'
grep -q 'dashboard live index-dryrun.js' "$work/deploy-out" \
  || fail 'dashboard bundle check did not run'
[ ! -s "$work/ssh.log" ] || fail 'dry-run called ssh or curl'
