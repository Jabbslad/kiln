#!/bin/sh
# Confirm every shipped client archive carries TAG.
# Linux is executed. macOS and Windows are checked by the version slot,
# because the release runner cannot execute those binaries.
set -eu

fail() {
  printf 'check-stamped-clients: %s\n' "$*" >&2
  exit 1
}

if [ "$#" -ne 2 ]; then
  fail 'usage: check-stamped-clients.sh DIST TAG'
fi

dist=$1
tag=$2
printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "tag ${tag}"
version=${tag#v}
[ -d "$dist" ] || fail 'dist is not a directory'

slot_version() {
  file=$1
  matches=$(LC_ALL=C grep -a -o -b -F 'KILNRELSLOT=' "$file" || true)
  [ -n "$matches" ] || fail "no version slot in ${file}"
  count=$(printf '%s\n' "$matches" | wc -l | awk '{ print $1 }')
  [ "$count" = 1 ] || fail "${file} has ${count} version slots"
  offset=${matches%%:*}
  case $offset in
    '' | *[!0-9]*) fail "bad slot offset in ${file}" ;;
  esac
  dd if="$file" bs=1 skip=$((offset + 12)) count=32 status=none | tr -d '\000'
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

check_one() {
  archive=$1
  member=$2
  execute=$3
  [ -f "$dist/$archive" ] || fail "missing ${archive}"
  rm -rf "$work/pkg"
  mkdir -p "$work/pkg"
  case $archive in
    *.tar.gz) tar -xzf "$dist/$archive" -C "$work/pkg" "$member" ;;
    *.zip) unzip -q "$dist/$archive" "$member" -d "$work/pkg" ;;
    *) fail "unexpected archive ${archive}" ;;
  esac
  [ -f "$work/pkg/$member" ] || fail "${archive} is missing ${member}"
  got=$(slot_version "$work/pkg/$member")
  [ "$got" = "$version" ] || fail "${archive} slot is ${got:-empty}"
  if [ "$execute" = 1 ]; then
    chmod +x "$work/pkg/$member"
    reported=$("$work/pkg/$member" --version | tr -d '\r')
    [ "$reported" = "kiln ${version}" ] || fail "${archive} reports ${reported:-empty}"
  fi
  printf 'client %s %s\n' "$tag" "$archive"
}

check_one "kiln-${tag}-x86_64-unknown-linux-gnu.tar.gz" kiln 1
check_one "kiln-${tag}-aarch64-apple-darwin.tar.gz" kiln 0
check_one "kiln-${tag}-x86_64-apple-darwin.tar.gz" kiln 0
check_one "kiln-${tag}-x86_64-pc-windows-msvc.zip" kiln.exe 0
