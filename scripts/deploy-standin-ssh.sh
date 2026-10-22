#!/bin/bash
# SSH/SCP to the disposable stand-in host (local port, generated key).
set -euo pipefail

fail() { printf 'deploy-standin-ssh: %s\n' "$*" >&2; exit 1; }

host=${KILN_STANDIN_SSH_HOST:-127.0.0.1}
port=${KILN_STANDIN_SSH_PORT:-}
key=${KILN_STANDIN_SSH_KEY:-}
user=${KILN_STANDIN_SSH_USER:-root}
[ -n "$port" ] || fail 'KILN_STANDIN_SSH_PORT is empty'
[ -n "$key" ] || fail 'KILN_STANDIN_SSH_KEY is empty'
[ -f "$key" ] || fail "missing stand-in key: ${key}"

target="${user}@${host}"
common=(
  -o StrictHostKeyChecking=no
  -o UserKnownHostsFile=/dev/null
  -o GlobalKnownHostsFile=/dev/null
  -o IdentitiesOnly=yes
  -o "IdentityFile=${key}"
  -o PreferredAuthentications=publickey
  -o PasswordAuthentication=no
  -o BatchMode=yes
  -o ConnectTimeout=20
  -o LogLevel=ERROR
  -p "$port"
)

multiplex=()
if [ -n "${KILN_SSH_CONTROL_PATH:-}" ]; then
  multiplex=(
    -o ControlMaster=auto
    -o "ControlPath=${KILN_SSH_CONTROL_PATH}"
    -o ControlPersist=600
  )
fi

mode=${1:?usage: deploy-standin-ssh.sh master-open|master-close|ssh|scp ...}
shift
case "$mode" in
  master-open)
    [ -n "${KILN_SSH_CONTROL_PATH:-}" ] || fail 'KILN_SSH_CONTROL_PATH is empty'
    ssh -M -f -N "${common[@]}" -o ControlMaster=yes \
      -o "ControlPath=${KILN_SSH_CONTROL_PATH}" -o ControlPersist=600 "$target"
    ;;
  master-close)
    ssh -O exit "${common[@]}" "${multiplex[@]}" "$target" 2>/dev/null || true
    ;;
  ssh)
    ssh -T "${common[@]}" "${multiplex[@]}" "$target" "$@"
    ;;
  scp)
    [ "$#" = 2 ] || fail 'usage: deploy-standin-ssh.sh scp LOCAL REMOTE_PATH'
    scp -P "$port" "${common[@]}" "${multiplex[@]}" "$1" "${target}:$2"
    ;;
  *) fail 'usage: deploy-standin-ssh.sh master-open|master-close|ssh|scp ...' ;;
esac
