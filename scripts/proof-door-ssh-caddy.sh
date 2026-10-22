#!/bin/sh
# Regenerate committed before/after transcripts for the door SSH Caddy edge fix.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
command -v caddy >/dev/null 2>&1 || bash scripts/install-caddy.sh
cargo test --locked -p kiln-server --test door_caddy door_ssh_caddy_proof_transcript -- --exact --nocapture
printf 'proof door-ssh-caddy: wrote deploy/proof/door-ssh-caddy/before-curl.txt and after-curl.txt\n'
