#!/bin/sh
# Exit 0 when TAG is v plus the Cargo.toml workspace version.
set -eu
if [ "$#" -ne 2 ]; then
  echo "usage: check-tag-version.sh TAG CARGO_VERSION" >&2
  exit 2
fi
tag=$1
cargo=$2
case $tag in
  v*)
    release=${tag#v}
    ;;
  *)
    echo "tag ${tag} has no leading v" >&2
    exit 1
    ;;
esac
if [ -z "$release" ] || [ "$release" != "$cargo" ]; then
  echo "tag ${tag} is ${release}, Cargo.toml is ${cargo}" >&2
  exit 1
fi
