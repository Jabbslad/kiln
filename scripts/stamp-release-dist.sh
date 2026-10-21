#!/bin/sh
# Rename reused release archives from FROM_TAG to TO_TAG and stamp binaries.
# FROM_TAG and TO_TAG are vX.Y.Z. Checksums are rewritten to match.
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: stamp-release-dist.sh DIST FROM_TAG TO_TAG" >&2
  exit 2
fi

dist=$1
from=$2
to=$3
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

tag_ok() {
  printf '%s\n' "$1" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$'
}

tag_ok "$from" || {
  echo "refusing from-tag ${from}" >&2
  exit 1
}
tag_ok "$to" || {
  echo "refusing to-tag ${to}" >&2
  exit 1
}
if [ "$from" = "$to" ]; then
  exit 0
fi
if [ ! -d "$dist" ]; then
  echo "dist is not a directory" >&2
  exit 1
fi
version=${to#v}

fail() {
  echo "stamp-release-dist: $*" >&2
  exit 1
}

rewrite_release() {
  file=$1
  tmp=$(mktemp)
  # Package jobs write serde_json's compact form ("version":"vX.Y.Z").
  # A single space after the colon is accepted too.
  if ! awk -v from="$from" -v to="$to" '
    BEGIN {
      olds[0] = "\"version\":\"" from "\""
      news[0] = "\"version\":\"" to "\""
      olds[1] = "\"version\": \"" from "\""
      news[1] = "\"version\": \"" to "\""
      count = 0
    }
    {
      rest = $0
      line = ""
      while (1) {
        best = 0
        bi = 0
        found = 0
        for (k = 0; k < 2; k++) {
          i = index(rest, olds[k])
          if (i > 0 && (found == 0 || i < bi)) {
            bi = i
            best = k
            found = 1
          }
        }
        if (found == 0) {
          line = line rest
          break
        }
        count++
        line = line substr(rest, 1, bi - 1) news[best]
        rest = substr(rest, bi + length(olds[best]))
      }
      print line
    }
    END { if (count != 1) exit 1 }
  ' "$file" >"$tmp"; then
    rm -f "$tmp"
    fail "release.json version is not ${from}"
  fi
  mv "$tmp" "$file"
}

stamp_tree() {
  root=$1
  list=$(mktemp)
  find "$root" -type f | sort >"$list"
  while IFS= read -r path; do
    rel=${path#"$root"/}
    case $rel in
      /*|*..*) fail "refusing path ${rel}" ;;
    esac
    case $rel in
      kiln|bin/kiln|bin/kiln-runtime|bin/kiln-host|bin/kiln-api|bin/kiln-identity)
        sh "$script_dir/stamp-release-version.sh" "$path" "$version" || exit 1
        ;;
    esac
  done <"$list"
  rm -f "$list"
  if [ -f "$root/release.json" ]; then
    rewrite_release "$root/release.json"
  fi
}

pack_tar() {
  work=$1
  dest=$2
  list=$(mktemp)
  (cd "$work" && find . -type f | sed 's|^\./||' | sort) >"$list"
  # mtime 1, not 0, so a repack cannot keep the digest main already attested.
  tar --create --gzip --file "$dest" \
    --owner=0 --group=0 --numeric-owner --mtime=@1 \
    -C "$work" --files-from "$list"
  rm -f "$list"
}

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
count=0
for archive in "$dist"/*; do
  [ -e "$archive" ] || continue
  name=$(basename "$archive")
  case $name in
    *.sha256) continue ;;
    *.tar.gz) ;;
    *) fail "unexpected dist file ${name}" ;;
  esac
  case $name in
    *"$from"*) ;;
    *) fail "${name} is not named ${from}" ;;
  esac
  rest=${name#*"$from"}
  case $rest in
    *"$from"*) fail "${name} names ${from} twice" ;;
  esac
  newname=${name%%"$from"*}${to}${rest}
  work=$(mktemp -d)
  tar -xzf "$archive" -C "$work"
  if find "$work" -type l | grep -q .; then
    rm -rf "$work"
    fail "${name} contains a symlink"
  fi
  stamp_tree "$work" || {
    rm -rf "$work"
    exit 1
  }
  dest="$out/$newname"
  pack_tar "$work" "$dest"
  rm -rf "$work"
  digest=$(sha256sum "$dest" | awk 'NR == 1 { print $1 }')
  printf '%s  %s\n' "$digest" "$newname" >"$out/$newname.sha256"
  count=$((count + 1))
done
if [ "$count" = 0 ]; then
  fail "dist has no archives"
fi
find "$dist" -type f \( -name '*.tar.gz' -o -name '*.sha256' \) -exec rm -f {} +
mv "$out"/* "$dist"/
