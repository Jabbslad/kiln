#!/bin/bash
# Download one published server release and install it over SSH.
set -euo pipefail
unset CDPATH

script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
ssh_sh=${KILN_SSH_SH:-$script_dir/ssh.sh}
# shellcheck source=/dev/null
source "$script_dir/guard.sh"
# A dry run checks the files deploy would copy. It does not SSH.
if [ "${KILN_DEPLOY_DRY_RUN:-}" != 1 ]; then
  guard_github_event deploy
fi

tag=${KILN_RELEASE_TAG:-latest}
address=${KILN_ADDRESS:-}
repo_root=$(cd -- "$script_dir/../.." && pwd)

if [ "$tag" = latest ]; then
  tag=$(
    curl --fail --silent --show-error --proto '=https' --proto-redir '=https' \
      --retry 3 \
      https://api.github.com/repos/Jabbslad/kiln/releases?per_page=20 \
      | awk '
        /"tag_name"/ {
          value = $0
          sub(/.*"tag_name"[[:space:]]*:[[:space:]]*"/, "", value)
          sub(/".*/, "", value)
          found = value
        }
        /"draft"/ {
          draft = ($0 ~ /true/)
          if (found != "" && draft == 0) { print found; exit }
          found = ""
        }
      '
  )
  [ -n "$tag" ] || fail 'no published release'
fi
printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$' \
  || fail "invalid release tag: ${tag}"
if [ -n "$address" ]; then
  printf '%s\n' "$address" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$' \
    || fail 'address must be an IPv4 literal'
fi
# v0.4.4's published archive rejects Debian 13, so that one tag runs this tree's
# install.sh. Newer stable releases run the installer from their own archive.
# Older tags and pre-releases stop here, before any download or SSH.
case $tag in
  v[0-9]*) ;;
  *) fail "refusing ${tag}; deploy accepts v0.4.4 and newer stable releases" ;;
esac
release=${tag#v}
case $release in
  *[!0-9.]*) fail "refusing ${tag}; deploy accepts v0.4.4 and newer stable releases" ;;
esac
IFS=. read -r major minor patch extra <<<"$release"
if [ -n "${extra:-}" ] || [ -z "${major:-}" ] || [ -z "${minor:-}" ] || [ -z "${patch:-}" ]; then
  fail "refusing ${tag}; deploy accepts v0.4.4 and newer stable releases"
fi
if [ "$major" -lt 0 ] \
  || { [ "$major" -eq 0 ] && [ "$minor" -lt 4 ]; } \
  || { [ "$major" -eq 0 ] && [ "$minor" -eq 4 ] && [ "$patch" -lt 4 ]; }; then
  fail "refusing ${tag}; deploy accepts v0.4.4 and newer stable releases"
fi

asset="kiln-server-${tag}-x86_64-unknown-linux-gnu.tar.gz"
identity="kiln-identity-${tag}-x86_64-unknown-linux-gnu.tar.gz"
base="https://github.com/Jabbslad/kiln/releases/download/${tag}"
work=$(mktemp -d)
remote=
cleanup() {
  rm -rf "$work"
  if [ -n "$remote" ]; then
    if [ "${KILN_DEPLOY_DRY_RUN:-}" = 1 ]; then
      rm -rf -- "$remote"
    else
      bash "$ssh_sh" ssh -- rm -rf -- "$remote" || true
    fi
  fi
}
trap cleanup EXIT

fetch_asset() {
  local name=$1
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --retry 3 --output "$work/$name" "${base}/${name}"
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --retry 3 --output "$work/${name}.sha256" "${base}/${name}.sha256"
  (
    cd "$work"
    sha256sum -c "${name}.sha256"
  )
}

put() {
  if [ "${KILN_DEPLOY_DRY_RUN:-}" = 1 ]; then
    cp -- "$1" "$2"
  else
    bash "$ssh_sh" scp "$1" "$2"
  fi
}

if [ "${KILN_DEPLOY_DRY_RUN:-}" = 1 ]; then
  dist=${KILN_DEPLOY_DIST:-}
  [ -n "$dist" ] || fail 'dry run needs KILN_DEPLOY_DIST'
  for name in "$asset" "${asset}.sha256" "$identity" "${identity}.sha256"; do
    [ -f "$dist/$name" ] || fail "missing deploy input ${name}"
    cp -- "$dist/$name" "$work/$name"
  done
  remote=$(mktemp -d /tmp/kiln-deploy.XXXXXX)
else
  fetch_asset "$asset"
  fetch_asset "$identity"
  remote=$(bash "$ssh_sh" ssh -- mktemp -d /tmp/kiln-deploy.XXXXXX)
fi
case "$remote" in
  /tmp/kiln-deploy.[A-Za-z0-9]*) ;;
  *) fail "unexpected remote directory: ${remote}" ;;
esac
put "$work/$asset" "$remote/$asset"
put "$work/${asset}.sha256" "$remote/${asset}.sha256"
put "$work/$identity" "$remote/$identity"
put "$work/${identity}.sha256" "$remote/${identity}.sha256"
put "$script_dir/install-identity.sh" "$remote/install-identity.sh"
if [ "$tag" = "v0.4.4" ]; then
  put "$repo_root/deploy/install.sh" "$remote/installer.sh"
fi
put "$script_dir/install-remote.sh" "$remote/install-remote.sh"
put "$script_dir/smoke.sh" "$remote/smoke.sh"
if [ "${KILN_DEPLOY_DRY_RUN:-}" = 1 ]; then
  KILN_IDENTITY_CHECK_ONLY=1 bash "$remote/install-identity.sh" "$remote"
  KILN_INSTALL_CHECK_ONLY=1 bash "$remote/install-remote.sh" "$remote"
  printf 'deploy inputs %s\n' "$tag"
  exit 0
fi
if [ -n "$address" ]; then
  bash "$ssh_sh" ssh -- bash "$remote/install-remote.sh" "$remote" "$address"
else
  bash "$ssh_sh" ssh -- bash "$remote/install-remote.sh" "$remote"
fi
printf 'deployed %s\n' "$tag"
