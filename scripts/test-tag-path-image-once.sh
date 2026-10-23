#!/bin/sh
# shellcheck disable=SC2016 # patterns match script source and must not expand
# Off CI, the PR tag-path rehearsal extracts rootfs.ext4 once and compiles
# release bins once. CI still requires TAG_PATH_RELEASE_BIN_DIR and still
# packages with package-server-release.sh.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
script=$root/scripts/tag-path-dry-run.sh
workflow=$root/.github/workflows/build.yml
package=$root/scripts/package-server-release.sh

fail() {
  printf 'test-tag-path-image-once: %s\n' "$*" >&2
  exit 1
}

[ -f "$script" ] || fail 'missing tag-path-dry-run.sh'

count() {
  grep -F -c "$1" "$2" || true
}

extracts=$(count 'tar -xzf "$prev_archive" -C "$image_root" image' "$script")
[ "$extracts" -eq 1 ] || fail "rootfs extract count is ${extracts}, want 1"

roots=$(count 'image_root=$(mktemp -d)' "$script")
[ "$roots" -eq 1 ] || fail "image_root mktemp count is ${roots}, want 1"

bins=$(count 'cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client' "$script")
[ "$bins" -eq 1 ] || fail "release bins compile count is ${bins}, want 1"

grep -F -q 'trap '"'"'rm -rf "$dist" "$output" "$trust_json" "$trust_bins" "$image_root"' "$script" \
  || fail 'EXIT trap must remove the first image_root'
grep -F -q 'rm -rf "$image_root"' "$script" \
  || fail 'success path must remove image_root'
grep -F -q -- '--image-dir "$image_root/image"' "$script" \
  || fail 'PR server package must use the single image dir'
grep -F -q 'KILN_PACKAGE_IMAGE_DIR="$image_root/image"' "$script" \
  || fail 'proved package must use the single image dir'

identity=$(grep -n 'cargo build --release --locked -p kiln-identity' "$script" | awk -F: 'NR == 1 { print $1 }')
checkout=$(grep -n 'git checkout -- Cargo.toml Cargo.lock' "$script" | awk -F: 'NR == 1 { print $1 }')
[ -n "$identity" ] || fail 'missing off-CI kiln-identity build'
[ -n "$checkout" ] || fail 'missing Cargo.toml checkout'
[ "$identity" -lt "$checkout" ] || fail 'kiln-identity must be built before git checkout restores Cargo.toml'

guard=$(awk -v n="$identity" 'NR < n && index($0, "if [ -z \"${GITHUB_ACTIONS:-}\" ]; then") { line = NR } END { print line }' "$script")
[ -n "$guard" ] || fail 'kiln-identity build must be guarded by an empty GITHUB_ACTIONS'
between=$(awk -v a="$guard" -v b="$identity" 'NR > a && NR < b && $1 == "fi" { c++ } END { print c + 0 }' "$script")
[ "$between" -eq 0 ] || fail 'kiln-identity build must sit inside the off-CI GITHUB_ACTIONS guard'

after=$(awk -v n="$checkout" 'NR > n && /cargo build/ { c++ } END { print c + 0 }' "$script")
[ "$after" -eq 0 ] || fail 'git checkout must not be followed by another cargo build'

grep -F -q 'stamp-release-version.sh "$reuse_owned/$name" "$from"' "$script" \
  || fail 'off-CI proved package must stamp copies to the from-tag'
if grep -F -q 'stamp-release-version.sh "target/release' "$script"; then
  fail 'stamp must not rewrite the binaries already checked at the next tag'
fi
stamp=$(grep -n 'stamp-release-version.sh "$reuse_owned/$name" "$from"' "$script" | awk -F: 'NR == 1 { print $1 }')
[ "$stamp" -gt "$checkout" ] || fail 'from-tag stamp must run after Cargo.toml is restored'

grep -F -q "fail 'TAG_PATH_RELEASE_BIN_DIR is required on pull requests'" "$script" \
  || fail 'CI must still fail closed without TAG_PATH_RELEASE_BIN_DIR'
grep -F -q 'PACKAGE_OUTPUT=dist sh scripts/package-server-release.sh' "$workflow" \
  || fail 'CI server package command changed'
grep -F -q 'TAG_PATH_RELEASE_BIN_DIR:' "$workflow" \
  || fail 'CI tag-path job must still pass TAG_PATH_RELEASE_BIN_DIR'
grep -F -q 'cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client' "$package" \
  || fail 'package-server-release.sh must still compile when CI calls it'
grep -F -q 'SKIP_RELEASE_BUILD' "$package" \
  || fail 'package-server-release.sh must still honor SKIP_RELEASE_BUILD'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src/image"
printf 'rootfs-bytes\n' >"$work/src/image/rootfs.ext4"
tar -czf "$work/archive.tar.gz" -C "$work/src" image
mkdir -p "$work/a" "$work/b"
tar -xzf "$work/archive.tar.gz" -C "$work/a" image
before=$(cksum "$work/a/image/rootfs.ext4")
cat "$work/a/image/rootfs.ext4" >/dev/null
after=$(cksum "$work/a/image/rootfs.ext4")
[ "$before" = "$after" ] || fail 'reading rootfs.ext4 changed its bytes'
tar -xzf "$work/archive.tar.gz" -C "$work/b" image
cmp "$work/a/image/rootfs.ext4" "$work/b/image/rootfs.ext4" \
  || fail 'two extracts of the same archive differ'

{
  printf 'prefix '
  printf 'KILNRELSLOT='
  dd if=/dev/zero bs=1 count=32 2>/dev/null
  printf ' suffix'
} >"$work/original"
cp "$work/original" "$work/stamped"
orig_sum=$(cksum "$work/original")
orig_len=$(wc -c <"$work/original" | awk '{ print $1 }')
sh "$root/scripts/stamp-release-version.sh" "$work/stamped" v1.2.3
[ "$(cksum "$work/original")" = "$orig_sum" ] || fail 'stamping a copy changed the original'
[ "$(wc -c <"$work/stamped" | awk '{ print $1 }')" = "$orig_len" ] || fail 'stamp changed file length'
LC_ALL=C grep -a -q 'KILNRELSLOT=1.2.3' "$work/stamped" || fail 'copy was not stamped'

printf 'tag-path image once ok\n'
