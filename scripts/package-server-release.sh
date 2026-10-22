#!/bin/sh
# Build and package the server release archives into PACKAGE_OUTPUT (default: dist).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

output=${PACKAGE_OUTPUT:-dist}
mkdir -p "$output"

trust_json=$(mktemp)
cargo build --release --locked --bins -p kiln-server -p kiln-runtime -p kiln-client \
  --message-format=json >"$trust_json"
sh scripts/check-release-trust.sh --cargo-json "$trust_json" \
  target/release/kiln \
  target/release/kiln-runtime \
  target/release/kiln-host \
  target/release/kiln-api
rm -f "$trust_json"
trust_json=$(mktemp)
cargo build --release --locked -p kiln-identity --message-format=json >"$trust_json"
sh scripts/check-release-trust.sh --cargo-json "$trust_json" \
  target/release/kiln-identity
rm -f "$trust_json"
target/release/kiln-host --help
target/release/kiln-api --help
cargo xtask release-version check-binary --version "$VERSION" -- \
  target/release/kiln \
  target/release/kiln-runtime \
  target/release/kiln-host \
  target/release/kiln-api \
  target/release/kiln-identity
case "$(target/release/kiln-identity --help)" in
  *github-authorize-url*|*github-token-url*|*github-user-url*|*provider-ca-file*)
    echo "release kiln-identity must not accept a substitute provider" >&2
    exit 1
    ;;
esac

cargo xtask release identity --version "$VERSION" --target x86_64-unknown-linux-gnu \
  --bin-dir target/release --output "$output"
sudo apt-get update
sudo apt-get install --no-install-recommends -y fakeroot musl-tools e2fsprogs curl binutils xz-utils
rustup target add x86_64-unknown-linux-musl
bash images/build-ubuntu.sh images/output/release '' systemd_warm
cargo xtask release server --version "$VERSION" --target x86_64-unknown-linux-gnu \
  --bin-dir target/release --image-dir images/output/release --output "$output"
(cd "$output" && sha256sum -c ./*.sha256)
