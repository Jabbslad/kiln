#!/bin/bash
# Open an SSH session with the repository deploy key and a pinned host key.
set -euo pipefail

fail() { printf 'ssh: %s\n' "$*" >&2; exit 1; }

[ -n "${DARK_FORGE_SSH_KEY:-}" ] || fail 'DARK_FORGE_SSH_KEY is empty'
[ -n "${DARK_FORGE_KNOWN_HOSTS:-}" ] || fail 'DARK_FORGE_KNOWN_HOSTS is empty'
case "${DARK_FORGE_SSH_KEY}" in
  *'PRIVATE KEY'*) ;;
  *) fail 'DARK_FORGE_SSH_KEY is not a private key' ;;
esac

host=${DARK_FORGE_HOST:-dark-forge.dev}
user=${DARK_FORGE_USER:-root}
port=${DARK_FORGE_PORT:-22}
case "$host" in
  ''|*[!A-Za-z0-9.:-]*) fail 'invalid host' ;;
esac
case "$user" in
  ''|*[!A-Za-z0-9._-]*) fail 'invalid user' ;;
esac
case "$port" in
  ''|*[!0-9]*) fail 'invalid port' ;;
esac

runtime=$(mktemp -d)
trap 'rm -rf "$runtime"' EXIT
umask 077
printf '%s\n' "$DARK_FORGE_SSH_KEY" > "$runtime/id"
printf '%s\n' "$DARK_FORGE_KNOWN_HOSTS" > "$runtime/known_hosts"
chmod 600 "$runtime/id" "$runtime/known_hosts"

# CheckHostIP is off because the pin is the hostname line Jamie stored.
# A changed A record must still match that key, and an address line is not required.
common=(
  -o StrictHostKeyChecking=yes
  -o UserKnownHostsFile="$runtime/known_hosts"
  -o GlobalKnownHostsFile=/dev/null
  -o CheckHostIP=no
  -o UpdateHostKeys=no
  -o IdentitiesOnly=yes
  -o IdentityFile="$runtime/id"
  -o PreferredAuthentications=publickey
  -o PasswordAuthentication=no
  -o KbdInteractiveAuthentication=no
  -o BatchMode=yes
  -o ConnectTimeout=20
  -o LogLevel=ERROR
)

ssh_port=()
[ "$port" = 22 ] || ssh_port=(-p "$port")
scp_port=()
[ "$port" = 22 ] || scp_port=(-P "$port")
target="${user}@${host}"

multiplex=()
if [ -n "${KILN_SSH_CONTROL_PATH:-}" ]; then
  multiplex=(
    -o ControlMaster=auto
    -o "ControlPath=${KILN_SSH_CONTROL_PATH}"
    -o ControlPersist=600
  )
fi

master_retryable() {
  local rc=$1
  local err=$2
  [ "$rc" -eq 255 ] && return 0
  printf '%s\n' "$err" | grep -qiE 'kex_exchange_identification|connection reset by peer|Connection reset'
}

open_master() {
  [ -n "${KILN_SSH_CONTROL_PATH:-}" ] || fail 'KILN_SSH_CONTROL_PATH is empty'
  local master_common=(
    "${common[@]}"
    -o ControlMaster=yes
    -o "ControlPath=${KILN_SSH_CONTROL_PATH}"
    -o ControlPersist=600
  )
  local attempt=1
  local delay
  local err
  local rc
  while [ "$attempt" -le 4 ]; do
    err=$(
      ssh -M -f -N "${ssh_port[@]}" "${master_common[@]}" "$target" 2>&1 >/dev/null
    ) && return 0
    rc=$?
    if ! master_retryable "$rc" "$err"; then
      printf '%s\n' "$err" >&2
      return "$rc"
    fi
    if [ "$attempt" -eq 4 ]; then
      printf '%s\n' "$err" >&2
      return "$rc"
    fi
    case "$attempt" in
      1) delay=5 ;;
      2) delay=15 ;;
      *) delay=30 ;;
    esac
    sleep "$delay"
    attempt=$((attempt + 1))
  done
}

mode=${1:?usage: ssh.sh master-open|master-close|ssh|scp ...}
shift
case "$mode" in
  master-open)
    open_master
    ;;
  master-close)
    ssh -O exit "${ssh_port[@]}" "${common[@]}" "${multiplex[@]}" "$target" 2>/dev/null || true
    ;;
  ssh)
    ssh -T "${ssh_port[@]}" "${common[@]}" "${multiplex[@]}" "$target" "$@"
    ;;
  scp)
    [ "$#" = 2 ] || fail 'usage: ssh.sh scp LOCAL REMOTE_PATH'
    scp "${scp_port[@]}" "${common[@]}" "${multiplex[@]}" "$1" "${target}:$2"
    ;;
  *) fail 'usage: ssh.sh master-open|master-close|ssh|scp ...' ;;
esac
