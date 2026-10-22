#!/bin/sh
# Caddy on docker shaped like production: caddy:2, read-only config bind mount.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'deploy-standin-caddy: %s\n' "$*" >&2
  exit 1
}

PIN=172.18.0.3
GATEWAY=172.18.0.1
NET=${KILN_STANDIN_CADDY_NETWORK:-kiln-standin-paseo}
CONTAINER=${KILN_STANDIN_CADDY_CONTAINER:-kiln-standin-caddy}
IMAGE=${KILN_STANDIN_CADDY_IMAGE:-caddy:2.10.2}
work=${KILN_STANDIN_WORK:-}
[ -n "$work" ] || fail 'KILN_STANDIN_WORK is empty'

docker_cmd() {
  if docker info >/dev/null 2>&1; then
    docker "$@"
    return
  fi
  sudo docker "$@"
}

mode=${1:?usage: deploy-standin-caddy.sh up|down|apply-door|apply-door-broken|edge-cert}
shift

caddyfile=$work/Caddyfile
ca=$work/ca.crt
edge_cert=$work/caddy-edge.crt
edge_key=$work/caddy-edge.key

ensure_edge_cert() {
  if [ -f "$edge_cert" ] && [ -f "$edge_key" ]; then
    return 0
  fi
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$edge_key" -out "$edge_cert" -days 1 \
    -subj '/CN=cli.dark-forge.dev' \
    -addext 'subjectAltName=DNS:cli.dark-forge.dev' >/dev/null 2>&1 \
    || fail 'could not create stand-in edge certificate'
}

write_door_site() {
  broken=$1
  door=$root/deploy/door.Caddyfile
  [ -f "$door" ] || fail "missing ${door}"
  ensure_edge_cert
  if [ "$broken" = 1 ]; then
    site=$(sed '/versions 1.1/d' "$door")
  else
    site=$(cat "$door")
  fi
  {
    printf '%s\n' 'cli.dark-forge.dev {'
    printf '\ttls /etc/caddy/public.crt /etc/caddy/public.key\n'
    printf '%s\n' "$site" | sed '1d;$d'
    printf '%s\n' '}'
  } >"$caddyfile"
}

case "$mode" in
  edge-cert)
    ensure_edge_cert
    ;;
  up)
    write_door_site 0
    touch "$ca"
    docker_cmd rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker_cmd network rm "$NET" >/dev/null 2>&1 || true
    docker_cmd network create --subnet=172.18.0.0/16 "$NET" >/dev/null
    docker_cmd run -d --name "$CONTAINER" --network "$NET" --ip "$PIN" \
      -p "127.0.0.1:443:443" \
      --mount "type=bind,source=${caddyfile},target=/etc/caddy/Caddyfile,readonly" \
      --mount "type=bind,source=${ca},target=/etc/kiln/ca.crt,readonly" \
      --mount "type=bind,source=${edge_cert},target=/etc/caddy/public.crt,readonly" \
      --mount "type=bind,source=${edge_key},target=/etc/caddy/public.key,readonly" \
      "$IMAGE" >/dev/null
    ;;
  down)
    docker_cmd rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker_cmd network rm "$NET" >/dev/null 2>&1 || true
    ;;
  apply-door)
    write_door_site 0
    [ -f "$ca" ] || fail "missing CA at ${ca}"
    docker_cmd exec "$CONTAINER" caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile \
      || fail 'caddy validate failed after door apply'
    docker_cmd exec "$CONTAINER" caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile \
      || fail 'caddy reload failed'
    ;;
  apply-door-broken)
    write_door_site 1
    [ -f "$ca" ] || fail "missing CA at ${ca}"
    docker_cmd exec "$CONTAINER" caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile \
      || fail 'caddy validate failed for broken door site'
    docker_cmd exec "$CONTAINER" caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile \
      || fail 'caddy reload failed for broken door site'
    ;;
  *) fail "unknown mode: ${mode}" ;;
esac

printf 'deploy-standin-caddy: %s ok\n' "$mode"
