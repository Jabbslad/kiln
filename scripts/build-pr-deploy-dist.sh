#!/bin/sh
# Build a full deploy dist from the current tree (server, identity, dashboard).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'build-pr-deploy-dist: %s\n' "$*" >&2
  exit 1
}

out=${1:-dist}
tag=${KILN_RELEASE_TAG:-}
[ -n "$tag" ] || fail 'KILN_RELEASE_TAG is required'
printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "invalid tag ${tag}"

VERSION=${tag#v}
export VERSION
cargo xtask release-version resolve --ref-type tag --ref-name "$tag"

trust_json=$(mktemp)
trap 'rm -f "$trust_json"' EXIT
cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client \
  --message-format=json >"$trust_json"
cargo xtask release-version check-binary --version "$tag" -- \
  target/release/kiln \
  target/release/kiln-runtime \
  target/release/kiln-host \
  target/release/kiln-api

cargo build --release --locked -p kiln-identity --message-format=json >"$trust_json"
cargo xtask release-version check-binary --version "$tag" -- target/release/kiln-identity

rm -rf -- "$out"
mkdir -p "$out"

cargo xtask release identity --version "$tag" --target x86_64-unknown-linux-gnu \
  --bin-dir target/release --output "$out"

if [ ! -d images/output/release/image ]; then
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    fakeroot musl-tools e2fsprogs curl binutils xz-utils >/dev/null 2>&1 || true
  rustup target add x86_64-unknown-linux-musl >/dev/null 2>&1 || true
  bash images/build-ubuntu.sh images/output/release '' systemd_warm
fi

cargo xtask release server --version "$tag" --target x86_64-unknown-linux-gnu \
  --bin-dir target/release --image-dir images/output/release --output "$out"

if [ -f web/package-lock.json ]; then
  (cd web && npm ci && npm run build)
fi
cargo xtask release dashboard --version "$tag" --target web --output "$out"

(cd "$out" && sha256sum -c ./*.sha256)
printf 'build-pr-deploy-dist: ok %s\n' "$tag"
