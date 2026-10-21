#!/bin/sh
# client job test steps for scripts/check.sh (includes stamp + toolchain)
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

ref_type=${REF_TYPE:-branch}
ref_name=${REF_NAME:-$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf 'main')}

cargo xtask release-version resolve --ref-type "$ref_type" --ref-name "$ref_name"
rustup show
bash scripts/ci/run-client-tests.sh
