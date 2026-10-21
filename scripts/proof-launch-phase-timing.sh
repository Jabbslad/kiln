#!/bin/sh
# Dashboard box.timing JSON before/after the launch-phase scope fix.
# Run from repo root: sh scripts/proof-launch-phase-timing.sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

cargo test -p kiln-runtime launch_phase_timing_cli_proof --locked -- --nocapture 2>&1 \
  | awk '/^PROOF_BEFORE / { sub(/^PROOF_BEFORE /, "before: "); print }
         /^PROOF_AFTER / { sub(/^PROOF_AFTER /, "after:  "); print }'
