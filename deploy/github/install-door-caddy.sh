#!/bin/bash
# Install deploy/door.Caddyfile into /opt/paseo/Caddyfile without restarting Caddy.
set -euo pipefail

fail() { printf 'install-door-caddy: %s\n' "$*" >&2; exit 1; }

site=${1:-}
[ -n "$site" ] || fail 'usage: install-door-caddy.sh DOOR_CADDYFILE'
[ -f "$site" ] || fail "missing door site: ${site}"

live=${KILN_PASEO_CADDYFILE:-/opt/paseo/Caddyfile}
backup=${KILN_PASEO_CADDYFILE_BAK:-/opt/paseo/Caddyfile.bak-deploy-door}
staging=${KILN_PASEO_CADDYFILE_NEW:-/opt/paseo/Caddyfile.new-deploy-door}
ca=${KILN_KILN_CA_CERT:-/etc/kiln/ca.crt}

[ -f "$live" ] || fail "missing live Caddyfile: ${live}"
[ -f "$ca" ] || fail "missing CA for Caddy validate: ${ca}"

docker_cmd=${KILN_DOCKER:-docker}
if ! "$docker_cmd" inspect -f '{{.State.Running}}' caddy 2>/dev/null | grep -qx true; then
  fail 'caddy container is not running'
fi
image=$("$docker_cmd" inspect -f '{{.Config.Image}}' caddy)

validate_config() {
  local config=$1
  local out rc
  rc=0
  out=$(
    "$docker_cmd" run --rm \
      --mount "type=bind,source=${config},target=/etc/caddy/Caddyfile,readonly" \
      --mount "type=bind,source=${ca},target=/etc/kiln/ca.crt,readonly" \
      "$image" \
      caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>&1
  ) || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '%s\n' "$out" >&2
    return "$rc"
  fi
  printf '%s\n' "$out"
  return 0
}

reload_live() {
  local out rc
  rc=0
  out=$(
    "$docker_cmd" exec caddy \
      caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile 2>&1
  ) || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '%s\n' "$out" >&2
    return "$rc"
  fi
  printf '%s\n' "$out"
  return 0
}

restore_and_reload_backup() {
  cp -a "$backup" "$live"
  if validate_config "$live" >/dev/null; then
    reload_live >/dev/null || true
  fi
}

site_bytes=$(mktemp)
trap 'rm -f "$site_bytes" "$staging"' EXIT
awk '
  {
    line = $0
    sub(/\r$/, "", line)
    printf "%s\n", line
  }
' "$site" >"$site_bytes"
if [ -s "$site_bytes" ] && [ -n "$(tail -c1 "$site_bytes")" ]; then
  printf '\n' >>"$site_bytes"
fi

merge_door() {
  awk -v door="$site_bytes" '
function braces(line,    i, c, n) {
  n = 0
  for (i = 1; i <= length(line); i++) {
    c = substr(line, i, 1)
    if (c == "{") n++
    else if (c == "}") n--
  }
  return n
}
BEGIN {
  while ((getline line < door) > 0) door_text = door_text line "\n"
  close(door)
  found = 0
  skip = 0
  depth = 0
}
{
  raw = $0
  sub(/\r$/, "", raw)
  if (!skip && raw == "cli.dark-forge.dev {") {
    found = 1
    skip = 1
    depth = braces(raw)
    printf "%s", door_text
    if (depth <= 0) skip = 0
    next
  }
  if (skip) {
    depth += braces(raw)
    if (depth <= 0) skip = 0
    next
  }
  print raw
}
END { exit !found }
' "$live"
}

if merge_door >"$staging"; then
  :
else
  cp -a "$live" "$staging"
  if [ -s "$staging" ] && [ -n "$(tail -c1 "$staging")" ]; then
    printf '\n' >>"$staging"
  fi
  cat "$site_bytes" >>"$staging"
fi

if cmp -s "$live" "$staging"; then
  printf 'door caddy unchanged\n'
  exit 0
fi

cp -a "$live" "$backup"

validate_rc=0
validate_out=$(validate_config "$staging") || validate_rc=$?
if [ "$validate_rc" -ne 0 ]; then
  rm -f "$staging"
  printf 'install-door-caddy: caddy validate failed (exit %s) on staging config\n' "$validate_rc" >&2
  exit 1
fi

cp -a "$staging" "$live"
rm -f "$staging"

reload_rc=0
reload_out=$(reload_live) || reload_rc=$?
if [ "$reload_rc" -ne 0 ]; then
  restore_and_reload_backup
  fail "caddy reload failed (exit ${reload_rc}): ${reload_out}"
fi

printf 'door caddy installed and reloaded\n'
printf '%s\n' "$validate_out"
printf '%s\n' "$reload_out"
