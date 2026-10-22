#!/bin/sh
# Disposable systemd stand-in host (jrei/systemd-ubuntu) with local SSH.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'deploy-standin-host: %s\n' "$*" >&2
  exit 1
}

CONTAINER=${KILN_STANDIN_CONTAINER:-kiln-standin-host}
IMAGE=${KILN_STANDIN_IMAGE:-jrei/systemd-ubuntu:24.04}
PORT=${KILN_STANDIN_SSH_PORT:-2222}
work=${KILN_STANDIN_WORK:-}
[ -n "$work" ] || fail 'KILN_STANDIN_WORK is empty'

docker_cmd() {
  if docker info >/dev/null 2>&1; then
    docker "$@"
    return
  fi
  sudo docker "$@"
}

mode=${1:?usage: deploy-standin-host.sh up|down|hosts|seed}
shift

case "$mode" in
  up)
    key=$work/standin-id
    pub=$work/standin-id.pub
    if [ ! -f "$key" ]; then
      ssh-keygen -t ed25519 -N '' -f "$key" >/dev/null
    fi
    docker_cmd rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker_cmd run -d --name "$CONTAINER" --privileged --cgroupns=host \
      --network host \
      -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
      -v /dev/kvm:/dev/kvm \
      "$IMAGE" >/dev/null
    deadline=$((SECONDS + 120))
    while [ "$SECONDS" -lt "$deadline" ]; do
      if docker_cmd exec "$CONTAINER" systemctl is-system-running >/dev/null 2>&1; then
        break
      fi
      sleep 2
    done
    docker_cmd exec "$CONTAINER" systemctl is-system-running >/dev/null \
      || fail 'stand-in systemd did not reach running'
    docker_cmd exec "$CONTAINER" bash -c \
      "DEBIAN_FRONTEND=noninteractive apt-get update -qq &&
       DEBIAN_FRONTEND=noninteractive apt-get install -y -qq openssh-server iproute2 curl ca-certificates tar openssl >/dev/null &&
       mkdir -p /run/sshd &&
       if ! grep -q '^Port ${PORT}' /etc/ssh/sshd_config; then
         printf '\nPort ${PORT}\n' >> /etc/ssh/sshd_config
       fi &&
       systemctl enable --now ssh" \
      || fail 'stand-in ssh install failed'
    docker_cmd exec "$CONTAINER" mkdir -p /root/.ssh
    docker_cmd exec "$CONTAINER" chmod 700 /root/.ssh
    docker_cmd cp "$pub" "$CONTAINER:/root/.ssh/authorized_keys"
    docker_cmd exec "$CONTAINER" chmod 600 /root/.ssh/authorized_keys
    printf '%s\n' "$PORT" >"$work/ssh-port"
    printf '%s\n' "$key" >"$work/ssh-key"
    ;;
  down)
    docker_cmd rm -f "$CONTAINER" >/dev/null 2>&1 || true
    ;;
  hosts)
    docker_cmd exec "$CONTAINER" bash -c \
      'grep -q cli.dark-forge.dev /etc/hosts || printf "127.0.0.1 cli.dark-forge.dev\n" >> /etc/hosts' \
      || fail 'could not set stand-in /etc/hosts'
    ;;
  seed)
    tag=${1:-}
    [ -n "$tag" ] || fail 'usage: deploy-standin-host.sh seed TAG'
    key=$(cat "$work/ssh-key")
    port=$(cat "$work/ssh-port")
    export KILN_STANDIN_SSH_KEY=$key
    export KILN_STANDIN_SSH_PORT=$port
    export KILN_SSH_SH=$root/scripts/deploy-standin-ssh.sh
    unset GITHUB_EVENT_NAME
    export KILN_SKIP_SMOKE=1
    KILN_RELEASE_TAG=$tag KILN_ADDRESS=127.0.0.1 bash "$root/deploy/github/deploy.sh" \
      || fail "seed deploy ${tag} failed"
    ;;
  *) fail "unknown mode: ${mode}" ;;
esac

printf 'deploy-standin-host: %s ok\n' "$mode"
