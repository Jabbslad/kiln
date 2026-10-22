#!/bin/sh
# version-guard stamps dist from FROM_TAG to TAG. package-server must ship FROM_TAG names.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-main-push-proved-package-stamp: %s\n' "$*" >&2
  exit 1
}

tag_script=$root/scripts/tag-path-dry-run.sh
stamp=$root/scripts/stamp-release-dist.sh
from=v0.4.17
to=v0.4.71

pack_minimal() {
  name=$1
  dir=$2
  archive=$work/dist/$name
  tar -C "$dir" -czf "$archive" .
  digest=$(sha256sum "$archive" | awk 'NR == 1 { print $1 }')
  printf '%s  %s\n' "$digest" "$name" >"${archive}.sha256"
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/dist" "$work/identity"
printf '{"target":"x86_64-unknown-linux-gnu","version":"v0.4.17"}\n' >"$work/identity/release.json"

pack_minimal kiln-identity-v0.4.71-x86_64-unknown-linux-gnu.tar.gz "$work/identity"
if sh "$stamp" "$work/dist" "$from" "$to" >"$work/out" 2>"$work/err"; then
  cat "$work/err" >&2
  fail 'stamp must reject main-push dist named at the planned tag'
fi
grep -q 'kiln-identity-v0.4.71-x86_64-unknown-linux-gnu.tar.gz is not named v0.4.17' "$work/err" \
  || fail "unexpected stamp error: $(cat "$work/err")"

proved_block=$(
  awk '/proved_dist=\$root\/dist-proved-package-server/{flag=1} flag && /^fi$/{print; exit} flag{print}' \
    "$tag_script"
)
[ -n "$proved_block" ] || fail 'tag-path-dry-run missing proved package-server block'

printf '%s\n' "$proved_block" | grep -q 'PACKAGE_OUTPUT=\$proved_dist sh scripts/package-server-release.sh' \
  || fail 'tag-path-dry-run must build proved package-server for main reuse'

if printf '%s\n' "$proved_block" | grep -q 'VERSION=\$tag'; then
  fail 'proved package-server must not use the planned tag in archive names'
fi
printf '%s\n' "$proved_block" | grep -q 'VERSION=\$from' \
  || fail 'proved package-server must set VERSION from the reuse from-tag'

printf '%s\n' "$proved_block" | grep -q 'git checkout -- Cargo.toml Cargo.lock' \
  || fail 'proved package-server must restore Cargo files before packaging at from-tag'

printf '%s\n' "$proved_block" | grep -q "sh scripts/stamp-release-dist.sh \"\$stamp_rehearsal\" \"\$from\" \"\$tag\"" \
  || fail 'tag-path-dry-run must rehearse version-guard stamp on proved package-server'

rm -rf "$work/dist"/*
mkdir -p "$work/identity"
printf '{"target":"x86_64-unknown-linux-gnu","version":"v0.4.17"}\n' >"$work/identity/release.json"
pack_minimal kiln-identity-v0.4.17-x86_64-unknown-linux-gnu.tar.gz "$work/identity"
if ! sh "$stamp" "$work/dist" "$from" "$to" >"$work/out" 2>"$work/err"; then
  cat "$work/err" >&2
  fail 'stamp must accept from-tagged dist for main version-guard'
fi
[ -f "$work/dist/kiln-identity-v0.4.71-x86_64-unknown-linux-gnu.tar.gz" ] \
  || fail 'stamp did not rename the archive to the planned tag'

printf 'main-push proved package stamp ok\n'
