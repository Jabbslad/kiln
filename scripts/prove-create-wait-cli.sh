#!/bin/sh
# Build the real kiln CLI and print before/after output for create vs create --wait.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo build --locked -p kiln-client --bin kiln
cargo test -p kiln-client create_wait_cli_before_after_proof -- \
  --exact \
  --nocapture \
  --test-threads=1
