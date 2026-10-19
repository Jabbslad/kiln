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

mode=${1:?usage: ssh.sh ssh|scp ...}
shift
case "$mode" in
  ssh)
    ssh_port=()
    [ "$port" = 22 ] || ssh_port=(-p "$port")
    ssh -T "${ssh_port[@]}" "${common[@]}" "${user}@${host}" "$@"
    ;;
  scp)
    [ "$#" = 2 ] || fail 'usage: ssh.sh scp LOCAL REMOTE_PATH'
    scp_port=()
    [ "$port" = 22 ] || scp_port=(-P "$port")
    scp "${scp_port[@]}" "${common[@]}" "$1" "${user}@${host}:$2"
    ;;
  *) fail 'usage: ssh.sh ssh|scp ...' ;;
esac
