#!/bin/bash
# Download one published server release and install it over SSH.
set -euo pipefail
unset CDPATH

script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
ssh_sh=${KILN_SSH_SH:-$script_dir/ssh.sh}
# shellcheck source=/dev/null
source "$script_dir/guard.sh"
guard_github_event deploy

tag=${KILN_RELEASE_TAG:-latest}
address=${KILN_ADDRESS:-}
repo_root=$(cd -- "$script_dir/../.." && pwd)

if [ "$tag" = latest ]; then
  tag=$(
    curl --fail --silent --show-error --proto '=https' --proto-redir '=https' \
      --retry 3 \
      https://api.github.com/repos/Jabbslad/kiln/releases?per_page=20 \
      | python3 -c 'import json, sys
releases = json.load(sys.stdin)
for release in releases:
    if not release.get("draft"):
        print(release["tag_name"])
        break
else:
    raise SystemExit("no published release")'
  )
fi
printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$' \
  || fail "invalid release tag: ${tag}"
if [ -n "$address" ]; then
  printf '%s\n' "$address" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$' \
    || fail 'address must be an IPv4 literal'
fi
# v0.4.4's archive installer rejects Debian 13, so that one tag runs this tree's
# install.py. Newer stable releases run the installer from their own archive.
# Older tags and pre-releases stop here, before any download or SSH.
python3 - "$tag" <<'PY' || fail "refusing ${tag}; deploy accepts v0.4.4 and newer stable releases"
import sys

tag = sys.argv[1]
if not tag.startswith("v") or "-" in tag:
    raise SystemExit(1)
parts = tag[1:].split(".")
if len(parts) != 3 or not all(part.isdigit() for part in parts):
    raise SystemExit(1)
if tuple(int(part) for part in parts) < (0, 4, 4):
    raise SystemExit(1)
PY

asset="kiln-server-${tag}-x86_64-unknown-linux-gnu.tar.gz"
base="https://github.com/Jabbslad/kiln/releases/download/${tag}"
work=$(mktemp -d)
remote=
cleanup() {
  rm -rf "$work"
  if [ -n "$remote" ]; then
    bash "$ssh_sh" ssh -- rm -rf -- "$remote" || true
  fi
}
trap cleanup EXIT

curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
  --retry 3 --output "$work/$asset" "${base}/${asset}"
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
  --retry 3 --output "$work/${asset}.sha256" "${base}/${asset}.sha256"
(
  cd "$work"
  sha256sum -c "${asset}.sha256"
)

remote=$(bash "$ssh_sh" ssh -- mktemp -d /tmp/kiln-deploy.XXXXXX)
case "$remote" in
  /tmp/kiln-deploy.[A-Za-z0-9]*) ;;
  *) fail "unexpected remote directory: ${remote}" ;;
esac
bash "$ssh_sh" scp "$work/$asset" "$remote/$asset"
bash "$ssh_sh" scp "$work/${asset}.sha256" "$remote/${asset}.sha256"
if [ "$tag" = "v0.4.4" ]; then
  bash "$ssh_sh" scp "$repo_root/deploy/install.py" "$remote/installer.py"
fi
bash "$ssh_sh" scp "$script_dir/install-remote.sh" "$remote/install-remote.sh"
bash "$ssh_sh" scp "$script_dir/smoke.sh" "$remote/smoke.sh"
if [ -n "$address" ]; then
  bash "$ssh_sh" ssh -- bash "$remote/install-remote.sh" "$remote" "$address"
else
  bash "$ssh_sh" ssh -- bash "$remote/install-remote.sh" "$remote"
fi
printf 'deployed %s\n' "$tag"
