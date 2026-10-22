#!/bin/sh
# Caddy on docker shaped like production: caddy:2, import /config/kiln/kiln.caddy.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'deploy-standin-caddy: %s\n' "$*" >&2
  exit 1
}

PIN=172.18.0.3
NET=${KILN_STANDIN_CADDY_NETWORK:-kiln-standin-paseo}
CONTAINER=${KILN_STANDIN_CADDY_CONTAINER:-caddy}
IMAGE=${KILN_STANDIN_CADDY_IMAGE:-caddy:2.10.2}
work=${KILN_STANDIN_WORK:-}
[ -n "$work" ] || fail 'KILN_STANDIN_WORK is empty'

paseo=${KILN_STANDIN_PASEO_CADDYFILE:-$work/paseo/Caddyfile}
snippet=${KILN_STANDIN_DOOR_SNIPPET:-${KILN_DOOR_CADDY_SNIPPET:-$work/kiln/kiln.caddy}}
import_line='import /config/kiln/kiln.caddy'

docker_cmd() {
  if docker info >/dev/null 2>&1; then
    docker "$@"
    return
  fi
  sudo docker "$@"
}

mode=${1:?usage: deploy-standin-caddy.sh up|down|apply-door|apply-door-broken|edge-cert}
shift

ca=$work/ca.crt
edge_cert=$work/caddy-edge.crt
edge_key=$work/caddy-edge.key
door_tls=$work/door-site.caddyfile

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

ensure_layout() {
  mkdir -p "$(dirname "$paseo")" "$(dirname "$snippet")"
  if [ ! -f "$paseo" ]; then
    printf '%s\n' "$import_line" >"$paseo"
  elif ! grep -Fq "$import_line" "$paseo"; then
    fail "stand-in paseo Caddyfile missing ${import_line}"
  fi
}

write_door_tls_site() {
  broken=$1
  door=$root/deploy/door.Caddyfile
  [ -f "$door" ] || fail "missing ${door}"
  ensure_edge_cert
  if [ "$broken" = 1 ]; then
    body=$(sed '/versions 1.1/d' "$door")
  else
    body=$(cat "$door")
  fi
  printf '%s\n' "$body" | awk '
    NR == 1 { print; print "\ttls /etc/caddy/public.crt /etc/caddy/public.key"; next }
    { print }
  ' >"$door_tls"
}

run_install_door() {
  broken=$1
  write_door_tls_site "$broken"
  ensure_layout
  [ -f "$snippet" ] || printf '%s\n' 'cli.dark-forge.dev { respond "seed" 200 }' >"$snippet"
  KILN_PASEO_CADDYFILE=$paseo \
    KILN_DOOR_CADDY_SNIPPET=$snippet \
    KILN_DOOR_CADDY_SKIP_PROBE=1 \
    bash "$root/deploy/github/install-door-caddy.sh" "$door_tls" \
    || fail 'install-door-caddy failed on the stand-in caddy container'
}

case "$mode" in
  edge-cert)
    ensure_edge_cert
    ;;
  up)
    ensure_layout
    ensure_edge_cert
    write_door_tls_site 0
    cp -a -- "$door_tls" "$snippet"
    touch "$ca"
    docker_cmd rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker_cmd network rm "$NET" >/dev/null 2>&1 || true
    docker_cmd network create --subnet=172.18.0.0/16 "$NET" >/dev/null
    docker_cmd run -d --name "$CONTAINER" --network "$NET" --ip "$PIN" \
      -p "127.0.0.1:443:443" \
      --mount "type=bind,source=${paseo},target=/etc/caddy/Caddyfile,readonly" \
      --mount "type=bind,source=${work}/kiln,target=/config/kiln" \
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
    [ -f "$ca" ] || fail "missing CA at ${ca}"
    run_install_door 0
    ;;
  apply-door-broken)
    [ -f "$ca" ] || fail "missing CA at ${ca}"
    run_install_door 1
    ;;
  *) fail "unknown mode: ${mode}" ;;
esac

printf 'deploy-standin-caddy: %s ok\n' "$mode"
