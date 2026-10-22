#!/bin/sh
# Run deploy final checks on a disposable stand-in host (PR CI).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'deploy-standin-final-checks: %s\n' "$*" >&2
  exit 1
}

if [ "${KILN_STANDIN_FAULT_INJECT:-}" = 1 ]; then
  fault=1
else
  fault=0
fi

work=$(mktemp -d)
trap 'sh "$root/scripts/deploy-standin-caddy.sh" down 2>/dev/null || true; sh "$root/scripts/deploy-standin-host.sh" down 2>/dev/null || true; rm -rf "$work"' EXIT
export KILN_STANDIN_WORK=$work
export KILN_STANDIN_PASEO_CADDYFILE=$work/paseo/Caddyfile
export KILN_STANDIN_DOOR_SNIPPET=$work/kiln/kiln.caddy

SEED_TAG=${KILN_STANDIN_SEED_TAG:-v0.4.61}
tags=$(git ls-remote --tags origin 'refs/tags/v*') || fail 'could not list tags'
pr_tag=$(printf '%s\n' "$tags" | sh "$root/scripts/next-release-tag.sh")
printf '%s\n' "$pr_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail "invalid pr tag ${pr_tag}"

if [ -c /dev/kvm ] && [ -r /dev/kvm ] 2>/dev/null; then
  skip_smoke=0
  printf 'deploy-standin-final-checks: /dev/kvm is available; smoke.sh will run\n'
else
  skip_smoke=1
  printf 'deploy-standin-final-checks: SKIPPED smoke.sh (/dev/kvm unavailable on this runner)\n' >&2
fi

if ! docker info >/dev/null 2>&1; then
  sudo systemctl start docker >/dev/null 2>&1 || true
fi
if ! docker info >/dev/null 2>&1; then
  if command -v dockerd >/dev/null 2>&1; then
    sudo dockerd >/tmp/kiln-dockerd.log 2>&1 &
    start=$(date +%s)
    deadline=$((start + 60))
    while [ "$(date +%s)" -lt "$deadline" ]; do
      docker info >/dev/null 2>&1 && break
      sleep 1
    done
  fi
  docker info >/dev/null 2>&1 || fail 'docker is not available for the stand-in caddy container'
fi

sh "$root/scripts/deploy-standin-host.sh" up || fail 'stand-in host did not start'
export KILN_STANDIN_SSH_KEY=$(cat "$work/ssh-key")
export KILN_STANDIN_SSH_PORT=$(cat "$work/ssh-port")
export KILN_SSH_SH=$root/scripts/deploy-standin-ssh.sh

unset GITHUB_EVENT_NAME
sh "$root/scripts/deploy-standin-host.sh" seed "$SEED_TAG" || fail 'seed install failed'
sh "$root/scripts/deploy-standin-host.sh" hosts || fail 'stand-in hosts file update failed'

docker_cmd() {
  if docker info >/dev/null 2>&1; then docker "$@"; else sudo docker "$@"; fi
}
docker_cmd cp "${KILN_STANDIN_CONTAINER:-kiln-standin-host}:/etc/kiln/ca.crt" "$work/ca.crt" \
  || fail 'could not copy kiln CA from stand-in'

# Trust the stand-in Caddy edge certificate inside the stand-in (production uses a public CA).
sh "$root/scripts/deploy-standin-caddy.sh" edge-cert || fail 'edge certificate generation failed'
docker_cmd cp "$work/caddy-edge.crt" \
  "${KILN_STANDIN_CONTAINER:-kiln-standin-host}:/usr/local/share/ca-certificates/kiln-standin-edge.crt"
docker_cmd exec "${KILN_STANDIN_CONTAINER:-kiln-standin-host}" update-ca-certificates >/dev/null \
  || fail 'stand-in could not trust the Caddy edge certificate'

if [ "$fault" -eq 1 ]; then
  sh "$root/scripts/deploy-standin-caddy.sh" up || fail 'caddy did not start for fault inject'
  sh "$root/scripts/deploy-standin-caddy.sh" apply-door-broken || fail 'broken door site did not load'
  if bash "$root/scripts/deploy-standin-ssh.sh" ssh -- \
    bash -s <"$root/deploy/github/check-door-ssh.sh"; then
    fail 'fault inject: check-door-ssh should fail when Caddy never got versions 1.1'
  fi
  printf 'deploy-standin-final-checks: fault injection rejected missing Caddy door fix (650f541-class)\n'
  exit 0
fi

sh "$root/scripts/deploy-standin-caddy.sh" up || fail 'caddy did not start'
sh "$root/scripts/deploy-standin-caddy.sh" apply-door || fail 'door.Caddyfile did not reach live Caddy'

KILN_RELEASE_TAG=$pr_tag sh "$root/scripts/build-pr-deploy-dist.sh" "$work/dist" || fail 'PR dist build failed'
export KILN_DEPLOY_STANDIN=1
export KILN_DEPLOY_DIST=$work/dist
export KILN_ADDRESS=127.0.0.1
if [ "$skip_smoke" -eq 1 ]; then
  export KILN_SKIP_SMOKE=1
fi

KILN_RELEASE_TAG=$pr_tag bash "$root/deploy/github/deploy.sh" || fail 'stand-in deploy failed'

printf 'deploy-standin-final-checks: ok %s\n' "$pr_tag"
