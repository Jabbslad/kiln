#!/bin/sh
# Install the repository pre-push hook, then allow the shell command.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

# beforeShellExecution passes the command as JSON on stdin.
cat >/dev/null

if ! bash scripts/install-hooks.sh >&2; then
  printf '%s\n' '{"permission":"deny","agent_message":"repository pre-push hook failed to install"}'
  exit 0
fi
printf '%s\n' '{"permission":"allow"}'
