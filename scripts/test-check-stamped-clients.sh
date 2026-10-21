#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check-stamped-clients: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

slot_bin() {
  path=$1
  version=$2
  name=$3
  mkdir -p "$(dirname "$path")"
  cat > "$path" << EOF
#!/bin/sh
if [ "\${1:-}" = --version ]; then
  printf '%s %s\\n' '$name' '$version'
  exit 0
fi
exit 2
EOF
  {
    printf 'KILNRELSLOT='
    printf '%s' "$version"
    dd if=/dev/zero bs=1 count=$((32 - ${#version})) status=none
  } >> "$path"
  chmod 755 "$path"
}

pack() {
  archive=$1
  member=$2
  version=$3
  label=$4
  stage=$work/stage
  rm -rf "$stage"
  mkdir -p "$stage"
  slot_bin "$stage/$member" "$version" "$label"
  case $archive in
    *.tar.gz) tar -C "$stage" -czf "$work/dist/$archive" "$member" ;;
    *) fail "unexpected archive ${archive}" ;;
  esac
}

mkdir -p "$work/dist"
pack kiln-v9.9.9-x86_64-unknown-linux-gnu.tar.gz kiln 9.9.9 kiln
pack kiln-v9.9.9-aarch64-apple-darwin.tar.gz kiln 9.9.9 kiln
pack kiln-v9.9.9-x86_64-apple-darwin.tar.gz kiln 9.9.9 kiln
sh "$root/scripts/check-stamped-clients.sh" "$work/dist" v9.9.9 >"$work/out"
grep -q 'client v9.9.9 kiln-v9.9.9-x86_64-unknown-linux-gnu.tar.gz' "$work/out" \
  || fail 'linux client was not checked'
grep -q 'client v9.9.9 kiln-v9.9.9-aarch64-apple-darwin.tar.gz' "$work/out" \
  || fail 'arm mac client was not checked'
grep -q 'client v9.9.9 kiln-v9.9.9-x86_64-apple-darwin.tar.gz' "$work/out" \
  || fail 'intel mac client was not checked'
if grep -E -q 'windows|kiln\.exe|\.zip' "$work/out"; then
  fail 'windows client is still a shipped asset'
fi
pack kiln-v9.9.9-x86_64-apple-darwin.tar.gz kiln 9.9.8 kiln
if sh "$root/scripts/check-stamped-clients.sh" "$work/dist" v9.9.9 >"$work/bad" 2>"$work/err"; then
  fail 'wrong mac slot was accepted'
fi
grep -q 'slot is 9.9.8' "$work/err" || fail "missing slot failure: $(cat "$work/err")"
printf 'stamped clients passed\n'
