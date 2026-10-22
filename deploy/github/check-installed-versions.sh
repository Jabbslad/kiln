#!/bin/bash
# Confirm every installed Kiln binary reports the release tag.
set -euo pipefail

fail() { printf 'installed-versions: %s\n' "$*" >&2; exit 1; }

tag=${1:?usage: check-installed-versions.sh TAG}
printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "tag ${tag}"
want=${tag#v}

prefix=${KILN_VERSION_CHECK_FIXTURE:-}

read_version() {
  local path=$1 name=$2 line
  line=$("$path" --version) || fail "${path} --version failed"
  printf '%s\n' "$line" | grep -Eq "^${name} [0-9]+\\.[0-9]+\\.[0-9]+$" \
    || fail "${path} --version printed ${line}"
  printf '%s\n' "${line#"${name}" }"
}

check_path() {
  local path=$1 name=$2 got
  [ -f "$path" ] || fail "missing ${path}"
  [ -L "$path" ] && fail "refusing symlink ${path}"
  got=$(read_version "$path" "$name") || fail "${path} --version failed"
  [ "$got" = "$want" ] || fail "${path} reports ${got}, want ${want}"
  printf 'binary %s %s\n' "$name" "$want"
}

if [ -n "${KILN_VERSION_CHECK_DIST:-}" ]; then
  dist=$KILN_VERSION_CHECK_DIST
  [ -d "$dist" ] || fail 'dist is not a directory'
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  server=
  identity=
  shopt -s nullglob
  for candidate in "$dist"/kiln-server-*.tar.gz; do
    [ -f "$candidate" ] || continue
    [ -z "$server" ] || fail 'more than one server archive'
    server=$candidate
  done
  for candidate in "$dist"/kiln-identity-*.tar.gz; do
    [ -f "$candidate" ] || continue
    [ -z "$identity" ] || fail 'more than one identity archive'
    identity=$candidate
  done
  shopt -u nullglob
  [ -n "$server" ] || fail 'missing server archive'
  [ -n "$identity" ] || fail 'missing identity archive'
  mkdir -p "$work/server/bin" "$work/identity/bin"
  tar -xzf "$server" -C "$work/server" bin/kiln bin/kiln-runtime bin/kiln-host bin/kiln-api
  tar -xzf "$identity" -C "$work/identity" bin/kiln-identity
  chmod 755 "$work/server/bin/"* "$work/identity/bin/kiln-identity"
  check_path "$work/server/bin/kiln" kiln
  check_path "$work/server/bin/kiln-runtime" kiln-runtime
  check_path "$work/server/bin/kiln-host" kiln-server
  check_path "$work/server/bin/kiln-api" kiln-server
  check_path "$work/identity/bin/kiln-identity" kiln-identity
  printf 'packaged versions %s\n' "$tag"
  exit 0
fi

check_path "$prefix/opt/kiln/bin/kiln" kiln
check_path "$prefix/opt/kiln/bin/kiln-runtime" kiln-runtime
check_path "$prefix/usr/local/libexec/kiln-host" kiln-server
check_path "$prefix/usr/local/libexec/kiln-api" kiln-server
check_path "$prefix/usr/local/libexec/kiln-identity" kiln-identity
printf 'installed versions %s\n' "$tag"
