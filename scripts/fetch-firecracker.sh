#!/usr/bin/env bash
# Download a pinned local toolchain. Never replaces system-installed binaries.
set -euo pipefail
output=${1:?usage: fetch-firecracker.sh OUTPUT_DIRECTORY}
mkdir -p "$output"
output=$(realpath "$output")
test ! -e "$output/firecracker" && test ! -e "$output/jailer" || { echo 'Refusing to overwrite binaries' >&2; exit 1; }
archive=$(mktemp -d "$output/.download.XXXXXX")
trap 'rm -rf -- "$archive"' EXIT
curl --fail --location --retry 3 \
  https://github.com/firecracker-microvm/firecracker/releases/download/v1.17.0/firecracker-v1.17.0-x86_64.tgz \
  -o "$archive/release.tgz"
printf '%s  %s\n' 06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558 "$archive/release.tgz" | sha256sum -c -
tar -xzf "$archive/release.tgz" -C "$archive"
for tool in firecracker jailer; do
  install -m 0755 "$archive/release-v1.17.0-x86_64/$tool-v1.17.0-x86_64" "$output/$tool"
done
# Print a command for the caller; keep its future PATH expansion literal.
# shellcheck disable=SC2016
printf 'Use explicitly: export PATH="%s:$PATH"\n' "$output"
