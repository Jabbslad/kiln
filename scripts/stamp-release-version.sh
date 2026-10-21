#!/bin/sh
# Write VERSION into the single KILNRELSLOT= value in FILE.
# The file length does not change. VERSION is X.Y.Z or vX.Y.Z.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: stamp-release-version.sh FILE VERSION" >&2
  exit 2
fi

file=$1
version=$2
case $version in
  v*) version=${version#v} ;;
esac
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' || {
  echo "refusing version ${version}" >&2
  exit 1
}

marker=KILNRELSLOT=
marker_len=${#marker}
value_len=32
if [ "${#version}" -ge "$value_len" ]; then
  echo "version does not fit the slot" >&2
  exit 1
fi
if [ -L "$file" ] || [ ! -f "$file" ]; then
  echo "refusing non-regular file ${file}" >&2
  exit 1
fi

matches=$(LC_ALL=C grep -a -o -b -F "$marker" "$file" || true)
if [ -z "$matches" ]; then
  echo "no version slot in ${file}" >&2
  exit 1
fi
count=$(printf '%s\n' "$matches" | wc -l | awk '{ print $1 }')
if [ "$count" != 1 ]; then
  echo "${file} has ${count} version slots" >&2
  exit 1
fi
offset=${matches%%:*}
case $offset in
  ''|*[!0-9]*)
    echo "bad slot offset in ${file}" >&2
    exit 1
    ;;
esac

payload=$(mktemp)
trap 'rm -f "$payload"' EXIT
{
  printf '%s' "$version"
  dd if=/dev/zero bs=1 count=$((value_len - ${#version})) 2>/dev/null
} >"$payload"
size=$(wc -c <"$payload" | awk '{ print $1 }')
if [ "$size" != "$value_len" ]; then
  echo "payload is ${size} bytes" >&2
  exit 1
fi
dd if="$payload" of="$file" bs=1 seek=$((offset + marker_len)) conv=notrunc status=none
