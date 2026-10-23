#!/bin/sh
# Fail when a text file names the dropped desktop platform.
# The slice method .windows( and the word executable are not references.
# Exceptions are relative paths in scripts/dropped-platform-allowlist.txt.
set -eu

fail() {
  printf 'check-dropped-platform: %s\n' "$*" >&2
  exit 1
}

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
default_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
root=${1:-$default_root}
root=$(CDPATH='' cd -- "$root" && pwd)
allowlist=$root/scripts/dropped-platform-allowlist.txt
[ -f "$allowlist" ] || fail 'missing scripts/dropped-platform-allowlist.txt'

entries=$(mktemp)
raw=$(mktemp)
hits=$(mktemp)
trap 'rm -f "$entries" "$raw" "$hits"' EXIT

while IFS= read -r entry || [ -n "$entry" ]; do
  case $entry in
    '' | '#'*) continue ;;
  esac
  case $entry in
    /* | *'..'*) fail "unsafe allowlist entry ${entry}" ;;
  esac
  case $entry in
    */)
      [ -d "$root/${entry%/}" ] || fail "allowlist directory missing: ${entry}"
      ;;
    *)
      [ -f "$root/$entry" ] || fail "allowlist file missing: ${entry}"
      ;;
  esac
  printf '%s\n' "$entry" >> "$entries"
done < "$allowlist"

allowed() {
  rel=$1
  while IFS= read -r entry; do
    case $entry in
      */)
        prefix=$entry
        rest=${rel#"$prefix"}
        [ "$rest" != "$rel" ] && return 0
        ;;
      *)
        [ "$rel" = "$entry" ] && return 0
        ;;
    esac
  done < "$entries"
  return 1
}

line_is_reference() {
  text=$1
  stripped=$(printf '%s\n' "$text" | sed 's/\.windows(//g')
  printf '%s\n' "$stripped" | grep -E -i -q 'windows|\.exe([^A-Za-z0-9_]|$)|powershell'
}

if grep -R -I -n -F -i \
  --exclude-dir=.git \
  --exclude-dir=target \
  --exclude-dir=node_modules \
  --exclude='*.log' \
  -e windows -e .exe -e powershell \
  "$root" > "$raw"
then
  while IFS= read -r line; do
    file=${line%%:*}
    rest=${line#*:}
    lineno=${rest%%:*}
    text=${rest#*:}
    case $file in
      "$root"/*) rel=${file#"$root"/} ;;
      *) fail "unexpected path ${file}" ;;
    esac
    if allowed "$rel"; then
      continue
    fi
    if line_is_reference "$text"; then
      printf '%s:%s:%s\n' "$rel" "$lineno" "$text" >> "$hits"
    fi
  done < "$raw"
fi

if [ -s "$hits" ]; then
  cat "$hits" >&2
  exit 1
fi
printf 'dropped platform check passed\n'
