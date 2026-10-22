#!/bin/bash
# Install deploy/door.Caddyfile into the Caddy config volume snippet and reload.
set -euo pipefail

fail() { printf 'install-door-caddy: %s\n' "$*" >&2; exit 1; }

site=${1:-}
[ -n "$site" ] || fail 'usage: install-door-caddy.sh DOOR_CADDYFILE'
[ -f "$site" ] || fail "missing door site: ${site}"

paseo_caddyfile=${KILN_PASEO_CADDYFILE:-/opt/paseo/Caddyfile}
snippet=${KILN_DOOR_CADDY_SNIPPET:-/var/lib/docker/volumes/paseo_caddy_config/_data/kiln/kiln.caddy}
import_line='import /config/kiln/kiln.caddy'

docker_cmd=${KILN_DOCKER:-docker}
caddy_config=/etc/caddy/Caddyfile

if ! "$docker_cmd" inspect -f '{{.State.Running}}' caddy 2>/dev/null | grep -qx true; then
  fail 'caddy container is not running'
fi

[ -f "$paseo_caddyfile" ] || fail "missing shared Caddyfile: ${paseo_caddyfile}"
if ! grep -Fq "$import_line" "$paseo_caddyfile"; then
  fail "one-time import edit required: add \"${import_line}\" to ${paseo_caddyfile}"
fi

[ -f "$snippet" ] || fail "missing door Caddy snippet at ${snippet}; Kiln Ops must move the cli.dark-forge.dev site there before deploy can update it"

if cmp -s "$site" "$snippet"; then
  printf 'door caddy unchanged\n'
  exit 0
fi

backup="${snippet}.bak-$(date -u +%Y%m%dT%H%M%SZ)"
cp -a "$snippet" "$backup"

install_snippet() {
  local src=$1
  local dest=$2
  local dir tmp
  dir=$(dirname "$dest")
  mkdir -p "$dir"
  tmp=$(mktemp "${dir}/kiln.caddy.XXXXXX")
  cp -a -- "$src" "$tmp"
  chmod 644 "$tmp"
  if [ "$(id -u)" -eq 0 ]; then
    chown root:root "$tmp"
  fi
  mv -f "$tmp" "$dest"
}

validate_whole() {
  local rc out
  rc=0
  out=$(
    "$docker_cmd" exec caddy \
      caddy validate --config "$caddy_config" --adapter caddyfile 2>&1
  ) || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '%s\n' "$out" >&2
    return "$rc"
  fi
  printf '%s\n' "$out"
  return 0
}

reload_live() {
  local rc out
  rc=0
  out=$(
    "$docker_cmd" exec caddy \
      caddy reload --config "$caddy_config" --adapter caddyfile 2>&1
  ) || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '%s\n' "$out" >&2
    return "$rc"
  fi
  printf '%s\n' "$out"
  return 0
}

restore_snippet() {
  install_snippet "$backup" "$snippet"
  reload_rc=0
  reload_out=$(reload_live) || reload_rc=$?
  if [ "$reload_rc" -ne 0 ]; then
    fail "restore reload failed (exit ${reload_rc}): ${reload_out}"
  fi
}

install_snippet "$site" "$snippet"

validate_rc=0
validate_out=$(validate_whole) || validate_rc=$?
if [ "$validate_rc" -ne 0 ]; then
  install_snippet "$backup" "$snippet"
  restore_rc=0
  restore_out=$(reload_live) || restore_rc=$?
  if [ "$restore_rc" -ne 0 ]; then
    fail "docker exec caddy caddy reload --config ${caddy_config} after validate failure failed (exit ${restore_rc}): ${restore_out}"
  fi
  fail "docker exec caddy caddy validate --config ${caddy_config} --adapter caddyfile failed (exit ${validate_rc}): ${validate_out}"
fi

reload_rc=0
reload_out=$(reload_live) || reload_rc=$?
if [ "$reload_rc" -ne 0 ]; then
  restore_snippet
  fail "docker exec caddy caddy reload --config ${caddy_config} --adapter caddyfile failed (exit ${reload_rc}): ${reload_out}"
fi

if [ "${KILN_DOOR_CADDY_SKIP_PROBE:-}" != 1 ]; then
  probe_rc=0
  probe_out=$(
    set +e
    app_code=$(curl --fail --silent --show-error --max-time 10 -o /dev/null -w '%{http_code}' \
      https://app.dark-forge.dev/ 2>&1)
    app_rc=$?
    login_code=$(curl --fail --silent --show-error --max-time 10 -o /dev/null -w '%{http_code}' \
      https://dark-forge.dev/login 2>&1)
    login_rc=$?
    cli_body=$(mktemp)
    cli_code=$(curl --silent --show-error --http1.1 --max-time 10 \
      --header 'Connection: Upgrade' \
      --header 'Upgrade: kiln-ssh' \
      --output "$cli_body" --write-out '%{http_code}' \
      https://cli.dark-forge.dev/v1/templates 2>&1)
    cli_rc=$?
    set -e
    if [ "$app_rc" -ne 0 ] || [ "$app_code" != 200 ]; then
      printf 'app.dark-forge.dev probe failed (curl exit %s, http %s)\n' "$app_rc" "$app_code"
      probe_rc=1
    fi
    if [ "$login_rc" -ne 0 ] || [ "$login_code" != 200 ]; then
      printf 'dark-forge.dev/login probe failed (curl exit %s, http %s)\n' "$login_rc" "$login_code"
      probe_rc=1
    fi
    if [ "$cli_rc" -ne 0 ] || [ "$cli_code" != 401 ]; then
      printf 'cli.dark-forge.dev Upgrade probe failed (curl exit %s, http %s)\n' "$cli_rc" "$cli_code"
      probe_rc=1
    fi
    rm -f "$cli_body"
    exit "$probe_rc"
  ) || probe_rc=$?
  if [ "$probe_rc" -ne 0 ]; then
    restore_snippet
    fail "post-reload HTTP probe failed: ${probe_out}"
  fi
fi

printf 'door caddy snippet installed and reloaded\n'
printf '%s\n' "$validate_out"
printf '%s\n' "$reload_out"
