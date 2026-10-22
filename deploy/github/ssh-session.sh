#!/bin/bash
# One multiplexed SSH connection per deploy or diagnose job.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
ssh_sh=${KILN_SSH_SH:-$script_dir/ssh.sh}

fail() { printf 'ssh-session: %s\n' "$*" >&2; exit 1; }

mode=${1:?usage: ssh-session.sh open|open-dry|close}
shift

case "$mode" in
  open-dry)
    [ -n "${KILN_SSH_CONTROL_PATH:-}" ] || fail 'KILN_SSH_CONTROL_PATH is empty'
    printf 'ssh-session: dry-run control path %s\n' "$KILN_SSH_CONTROL_PATH"
    ;;
  open)
    [ -n "${KILN_SSH_CONTROL_PATH:-}" ] || fail 'KILN_SSH_CONTROL_PATH is empty'
    bash "$ssh_sh" master-open
    ;;
  close)
    if [ -n "${KILN_SSH_CONTROL_PATH:-}" ]; then
      bash "$ssh_sh" master-close || true
    fi
    ;;
  *) fail 'usage: ssh-session.sh open|open-dry|close' ;;
esac
