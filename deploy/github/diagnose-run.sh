#!/bin/bash
# Collect the fixed diagnose report over SSH and keep a local copy for the artifact.
set -euo pipefail
unset CDPATH

script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
ssh_session=$script_dir/ssh-session.sh
# shellcheck source=/dev/null
source "$script_dir/guard.sh"
: > diagnose.log
guard_github_event diagnose
export KILN_SSH_CONTROL_PATH="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/kiln-ssh-control"
bash "$ssh_session" open
trap 'bash "$ssh_session" close' EXIT
bash "$script_dir/ssh.sh" ssh -- bash -s < "$script_dir/diagnose.sh" 2>&1 | tee diagnose.log
