#!/bin/sh
# Build and package the server release archives into PACKAGE_OUTPUT (default: dist).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

output=${PACKAGE_OUTPUT:-dist}
mkdir -p "$output"

bin_dir=${RELEASE_BIN_DIR:-target/release}
skip_build=${SKIP_RELEASE_BUILD:-0}

require_bin() {
  path=$1
  if [ ! -f "$path" ] || [ ! -x "$path" ]; then
    echo "package-server-release: missing release binary ${path}" >&2
    exit 1
  fi
}

if [ "$skip_build" = 1 ]; then
  for name in kiln kiln-runtime kiln-host kiln-api kiln-identity; do
    require_bin "$bin_dir/$name"
  done
else
  trust_json=$(mktemp)
  cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client \
    --message-format=json >"$trust_json"
  sh scripts/check-release-trust.sh --cargo-json "$trust_json" \
    "$bin_dir/kiln" \
    "$bin_dir/kiln-runtime" \
    "$bin_dir/kiln-host" \
    "$bin_dir/kiln-api"
  rm -f "$trust_json"
  trust_json=$(mktemp)
  cargo build --release --locked -p kiln-identity --message-format=json >"$trust_json"
  sh scripts/check-release-trust.sh --cargo-json "$trust_json" \
    "$bin_dir/kiln-identity"
  rm -f "$trust_json"
fi

"$bin_dir/kiln-host" --help
"$bin_dir/kiln-api" --help
cargo xtask release-version check-binary --version "$VERSION" -- \
  "$bin_dir/kiln" \
  "$bin_dir/kiln-runtime" \
  "$bin_dir/kiln-host" \
  "$bin_dir/kiln-api" \
  "$bin_dir/kiln-identity"
case "$("$bin_dir/kiln-identity" --help)" in
  *github-authorize-url*|*github-token-url*|*github-user-url*|*provider-ca-file*)
    echo "release kiln-identity must not accept a substitute provider" >&2
    exit 1
    ;;
esac

cargo xtask release identity --version "$VERSION" --target x86_64-unknown-linux-gnu \
  --bin-dir "$bin_dir" --output "$output"

if [ -n "${KILN_PACKAGE_IMAGE_DIR:-}" ]; then
  image_dir=$KILN_PACKAGE_IMAGE_DIR
else
  sudo apt-get update
  sudo apt-get install --no-install-recommends -y fakeroot musl-tools e2fsprogs curl binutils xz-utils
  rustup target add x86_64-unknown-linux-musl
  bash images/build-ubuntu.sh images/output/release '' systemd_warm
  image_dir=images/output/release
fi

cargo xtask release server --version "$VERSION" --target x86_64-unknown-linux-gnu \
  --bin-dir "$bin_dir" --image-dir "$image_dir" --output "$output"
(cd "$output" && sha256sum -c ./*.sha256)
