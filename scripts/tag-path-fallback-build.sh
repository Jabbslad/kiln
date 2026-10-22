#!/bin/sh
# Build the release dist locally at FROM_TAG (full tag-path fallback when reuse has no green main ancestor).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'tag-path-fallback-build: %s\n' "$*" >&2
  exit 1
}

from=${1:-}
dist=${2:-}
[ -n "$from" ] && [ -n "$dist" ] || fail 'usage: tag-path-fallback-build.sh FROM_TAG DIST_DIR'
printf '%s\n' "$from" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "from-tag ${from} is not stable"

from_commit=$(git rev-parse "${from}^{commit}" 2>/dev/null) \
  || fail "from-tag ${from} does not resolve to a commit"
mkdir -p "$dist"

git checkout "$from_commit" -- Cargo.toml Cargo.lock
VERSION=${from#v}
export VERSION
cargo xtask release-version resolve --ref-type tag --ref-name "$from"

PACKAGE_OUTPUT="$dist" sh scripts/package-server-release.sh

cargo build --release --locked -p kiln-client --bin kiln
cargo xtask release-version check-binary --version "$from" -- target/release/kiln
cargo xtask release client --version "$from" --target x86_64-unknown-linux-gnu \
  --bin-dir target/release --output "$dist"

rustup target add x86_64-apple-darwin aarch64-apple-darwin >/dev/null 2>&1 || true
cargo build --release --locked -p kiln-client --bin kiln --target x86_64-apple-darwin
cargo xtask release-version check-binary --version "$from" -- \
  target/x86_64-apple-darwin/release/kiln
cargo xtask release client --version "$from" --target x86_64-apple-darwin \
  --bin-dir target/x86_64-apple-darwin/release --output "$dist"

cargo build --release --locked -p kiln-client --bin kiln --target aarch64-apple-darwin
cargo xtask release-version check-binary --version "$from" -- \
  target/aarch64-apple-darwin/release/kiln
cargo xtask release client --version "$from" --target aarch64-apple-darwin \
  --bin-dir target/aarch64-apple-darwin/release --output "$dist"

(
  cd web
  npm ci
  npm run build
)
cargo xtask release dashboard --version "$from" --target web \
  --bin-dir web/dist --output "$dist"

git checkout -- Cargo.toml Cargo.lock
(cd "$dist" && sha256sum -c ./*.sha256)

printf 'tag-path-fallback-build: assembled dist for %s\n' "$from" >&2
