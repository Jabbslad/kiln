#!/bin/bash
# Replace /usr/local/libexec/kiln-identity from the archive in DIRECTORY.
# The previous executable is kept under /opt/kiln/upgrades. A failed probe
# puts that executable back. This script does not install a unit file.
set -euo pipefail
umask 077

fail() { printf 'identity: %s\n' "$*" >&2; exit 1; }

identity_step_fail() {
  local name=$1 log rc
  shift
  log=$(mktemp)
  set +e
  "$@" >"$log" 2>&1
  rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    rm -f -- "$log"
    return 0
  fi
  {
    printf 'identity step failed: %s\n' "$name"
    printf 'command: %s\n' "$*"
    printf 'exit code: %d\n' "$rc"
    printf 'last output lines:\n'
    tail -n 20 "$log"
  } >&2
  rm -f -- "$log"
  return "$rc"
}

identity_step_detail() {
  local name=$1 command=$2 rc=$3 detail=$4
  {
    printf 'identity step failed: %s\n' "$name"
    printf 'command: %s\n' "$command"
    printf 'exit code: %d\n' "$rc"
    printf 'last output lines:\n'
    printf '%s\n' "$detail"
  } >&2
  return "$rc"
}

identity_nrestarts() {
  local value
  value=$(show_prop NRestarts | tr -d '[:space:]')
  case "$value" in
    ''|*[!0-9]*)
      printf 'unknown\n' >&2
      return 1
      ;;
  esac
  printf '%s\n' "$value"
}

loopback_login_step() {
  loopback_ok
}

verify_signin_after_restart() {
  local restarts_before restarts_after
  restarts_before=$(identity_nrestarts) || restarts_before=unknown
  identity_step_fail "kiln-identity active after restart" \
    systemctl is-active --quiet kiln-identity.service || return 1
  identity_step_fail "kiln-identity login after restart" \
    loopback_login_step || return 1
  sleep 2
  restarts_after=$(identity_nrestarts) || restarts_after=unknown
  if [ "$restarts_before" != "$restarts_after" ]; then
    identity_step_detail "kiln-identity stable after restart" \
      "compare NRestarts (settle after restart)" 1 \
      "NRestarts before=${restarts_before} after=${restarts_after}" || return 1
  fi
}

restart_identity_service() {
  systemctl restart kiln-identity.service
  verify_signin_after_restart
}

dir=${1:?usage: install-identity.sh DIRECTORY}
if [ "${KILN_IDENTITY_CHECK_ONLY:-}" = 1 ]; then
  case "$dir" in
    /tmp/kiln-deploy.[A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9]) ;;
    *) fail 'refusing to operate outside the deploy directory' ;;
  esac
  prefix=
elif [ -n "${KILN_IDENTITY_FIXTURE:-}" ]; then
  prefix=${KILN_IDENTITY_FIXTURE}
else
  case "$dir" in
    /tmp/kiln-deploy.[A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9]) ;;
    *) fail 'refusing to operate outside the deploy directory' ;;
  esac
  [ "$(id -u)" -eq 0 ] || fail 'identity upgrade requires root'
  prefix=
fi

libexec=$prefix/usr/local/libexec
opt=$prefix/opt/kiln
proc=$prefix/proc
live=$libexec/kiln-identity
marker=$opt/upgrades/identity-in-progress
lock=$prefix/run/lock/kiln-install.lock
extract=

cleanup() {
  if [ -n "$extract" ]; then
    rm -rf "$extract"
  fi
}
trap cleanup EXIT

cd "$dir"

found=0
archive=
shopt -s nullglob
for candidate in kiln-identity-v[0-9]*.tar.gz; do
  if [ -f "$candidate" ]; then
    found=$((found + 1))
    archive=$candidate
  fi
done
shopt -u nullglob
[ "$found" -eq 1 ] || fail 'missing identity archive'
[ -f "${archive}.sha256" ] || fail 'missing identity checksum'

awk -v name="$archive" '
  NF != 2 || $2 != name || $1 !~ /^[0-9a-f]{64}$/ { bad = 1 }
  END { exit bad || NR != 1 }
' "${archive}.sha256" || fail 'unexpected checksum file'
sha256sum -c "${archive}.sha256"

names=$(mktemp)
types=$(mktemp)
tar -tzf "$archive" > "$names"
awk '
  /(^|\/)\.\.(\/|$)/ || /^\// || /\/$/ { bad = 1 }
  END { exit bad }
' "$names" || fail 'archive path is not safe'
tar -tvzf "$archive" > "$types"
awk 'substr($0, 1, 1) != "-" { bad = 1 } END { exit bad }' "$types" \
  || fail 'archive contains a non-regular file'
rm -f "$names" "$types"

extract=$(mktemp -d)
tar -xzf "$archive" -C "$extract"
[ -f "$extract/release.json" ] || fail 'archive is missing release.json'
[ -f "$extract/bin/kiln-identity" ] || fail 'archive is missing bin/kiln-identity'
chmod 755 "$extract/bin/kiln-identity"

filename_version=${archive#kiln-identity-v}
filename_version=${filename_version%%-*}
release_version=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\(v\{0,1\}[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)".*/\1/p' "$extract/release.json")
release_version=${release_version#v}
target=$(sed -n 's/.*"target"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$extract/release.json")
printf '%s\n' "$filename_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail 'invalid archive version'
[ "$release_version" = "$filename_version" ] || fail 'release.json version does not match the archive name'
[ "$target" = "x86_64-unknown-linux-gnu" ] || fail 'unexpected identity target'
package_version=$("$extract/bin/kiln-identity" --version) || fail 'packaged identity version failed'
[ "$package_version" = "kiln-identity $filename_version" ] || fail "packaged identity reports $package_version"
if [ "${KILN_IDENTITY_CHECK_ONLY:-}" = 1 ]; then
  printf 'identity inputs %s\n' "$filename_version"
  exit 0
fi
package=$extract/bin/kiln-identity
version=$filename_version

version_order() {
  awk -v left="$1" -v right="$2" 'BEGIN {
    split(left, a, ".")
    split(right, b, ".")
    for (i = 1; i <= 3; i++) {
      if ((a[i] + 0) < (b[i] + 0)) { print "older"; exit }
      if ((a[i] + 0) > (b[i] + 0)) { print "newer"; exit }
    }
    print "equal"
  }'
}

read_version() {
  local line
  line=$("$1" --version) || return 1
  printf '%s\n' "$line" | grep -Eq '^kiln-identity [0-9]+\.[0-9]+\.[0-9]+$' || return 1
  printf '%s\n' "${line#kiln-identity }"
}

show_prop() {
  systemctl show kiln-identity.service -p "$1" --value --no-pager
}

process_matches() {
  local pid link
  pid=$(show_prop MainPID)
  pid=$(printf '%s' "$pid" | tr -d '[:space:]')
  case "$pid" in
    ''|0|*[!0-9]*) return 1 ;;
  esac
  link=$(readlink "$proc/$pid/exe" 2>/dev/null || true)
  case "$link" in
    *'(deleted)') return 1 ;;
  esac
  [ "$link" = "$live" ]
}

atomic_copy() {
  local source=$1 destination=$2 dir tmp
  [ -L "$destination" ] && return 1
  dir=$(dirname "$destination")
  tmp=$(mktemp "$dir/.kiln-identity-XXXXXX")
  if ! cp -- "$source" "$tmp"; then
    rm -f -- "$tmp"
    return 1
  fi
  chmod 755 "$tmp" || { rm -f -- "$tmp"; return 1; }
  sync "$tmp" || { rm -f -- "$tmp"; return 1; }
  mv -f -- "$tmp" "$destination" || { rm -f -- "$tmp"; return 1; }
  sync "$dir"
}

loopback_ok() {
  local log_probe=${1:-} i=0 body code phrase=missing
  while [ "$i" -lt 5 ]; do
    body=$(mktemp)
    code=$(curl --silent --show-error --proto '=http' --proto-redir '=http' --max-time 10 \
      --output "$body" --write-out '%{http_code}' \
      http://127.0.0.1:8091/login || true)
    phrase=missing
    if grep -q 'Light the way in.' "$body"; then
      phrase=found
    fi
    rm -f "$body"
    if [ "$code" = 200 ] && [ "$phrase" = found ]; then
      if [ "$log_probe" = log ]; then
        printf 'identity probe loopback: status=%s phrase=%s\n' "$code" "$phrase"
      fi
      return 0
    fi
    i=$((i + 1))
    if [ "$i" -lt 5 ]; then
      sleep 1
    fi
  done
  if [ "$log_probe" = log ]; then
    printf 'identity probe loopback: status=%s phrase=%s\n' "${code:-000}" "$phrase"
  fi
  return 1
}

public_login_ok() {
  local body code phrase=missing
  body=$(mktemp)
  code=$(curl --silent --show-error --proto '=https' --proto-redir '=https' --max-time 10 \
    --output "$body" --write-out '%{http_code}' \
    https://dark-forge.dev/login || true)
  if grep -q 'Light the way in.' "$body"; then
    phrase=found
  fi
  rm -f "$body"
  printf 'identity probe public: status=%s phrase=%s\n' "${code:-000}" "$phrase"
  [ "$code" = 200 ] && [ "$phrase" = found ]
}

app_login_ok() {
  local code hdr oauth=no challenge=absent
  hdr=$(mktemp)
  code=$(curl --silent --show-error --proto '=https' --proto-redir '=https' --max-redirs 0 --max-time 10 \
    -D "$hdr" --output /dev/null --write-out '%{http_code}' \
    https://app.dark-forge.dev/login || true)
  if grep -qi '/oauth/server' "$hdr"; then
    oauth=yes
  fi
  if grep -q 'code_challenge_method=S256' "$hdr"; then
    challenge=S256
  fi
  rm -f "$hdr"
  printf 'identity probe app: status=%s location_oauth_server=%s code_challenge_method=%s\n' \
    "${code:-000}" "$oauth" "$challenge"
  [ "$code" = 303 ] && [ "$oauth" = yes ] && [ "$challenge" = S256 ]
}

probe_ok() {
  local have
  have=$(read_version "$live" || true)
  if [ "$have" = "$version" ]; then
    printf 'identity probe version: match %s\n' "$version"
  else
    printf 'identity probe version: mismatch have=%s want=%s\n' "${have:-unknown}" "$version"
  fi
  public_login_ok && app_login_ok
}

retire_marker() {
  local old suffix dest
  old=$(read_version "$marker/kiln-identity") || fail 'backup identity version failed'
  suffix=$(od -An -N4 -tx1 /dev/urandom | tr -d '[:space:]')
  dest=$opt/upgrades/identity-${old}-${suffix}
  [ ! -e "$dest" ] || fail 'backup name already exists'
  mv "$marker" "$dest"
  printf '%s\n' "$dest"
}

restore_marker() {
  local old dest
  old=$(read_version "$marker/kiln-identity") || old=unknown
  atomic_copy "$marker/kiln-identity" "$live" || fail 'could not restore the previous identity binary'
  systemctl restart kiln-identity.service
  if verify_signin_after_restart; then
    dest=$(retire_marker)
    printf 'identity: restored %s; previous %s\n' "$old" "$dest" >&2
  else
    printf 'identity: update and rollback failed\n' >&2
  fi
  return 1
}

mkdir -p "$(dirname "$lock")"
exec {LOCK_FD}<>"$lock"
flock -n "$LOCK_FD" || fail 'another installer is running'

if [ ! -f "$live" ] || [ -L "$live" ]; then
  fail 'identity binary is not installed'
fi
[ "$(show_prop ActiveState | tr -d '[:space:]')" = active ] || fail 'kiln-identity is not active'

if [ -d "$marker" ]; then
  if cmp -s "$package" "$live"; then
    retire_marker >/dev/null
  else
    restore_marker
    exit 1
  fi
fi

installed=$(read_version "$live") || fail 'installed identity version failed'
order=$(version_order "$installed" "$version")
if [ "$order" = newer ]; then
  fail "refusing downgrade from $installed to $version"
fi

mask_slot() {
  local src=$1 dest=$2 matches count offset
  cp -- "$src" "$dest"
  matches=$(LC_ALL=C grep -a -o -b -F 'KILNRELSLOT=' "$dest" || true)
  if [ -z "$matches" ]; then
    return 0
  fi
  count=$(printf '%s\n' "$matches" | wc -l | awk '{ print $1 }')
  if [ "$count" != 1 ]; then
    return 1
  fi
  offset=${matches%%:*}
  case $offset in
    ''|*[!0-9]*) return 1 ;;
  esac
  dd if=/dev/zero of="$dest" bs=1 seek=$((offset + 12)) count=32 conv=notrunc status=none
}

slot_equal() {
  local a b
  a=$(mktemp)
  b=$(mktemp)
  if ! mask_slot "$1" "$a" || ! mask_slot "$2" "$b"; then
    rm -f "$a" "$b"
    return 1
  fi
  if cmp -s "$a" "$b"; then
    rm -f "$a" "$b"
    return 0
  fi
  rm -f "$a" "$b"
  return 1
}

if slot_equal "$package" "$live"; then
  if ! process_matches; then
    restart_identity_service || fail 'identity sign-in check failed after restart'
    probe_ok || fail 'identity probe failed'
  fi
  printf 'identity unchanged %s\n' "$version"
  exit 0
fi

if [ ! -d "$opt" ] || [ -L "$opt" ]; then
  fail 'missing kiln opt directory'
fi
mkdir -p "$opt/upgrades"
chmod 700 "$opt/upgrades"
mkdir "$marker"
chmod 700 "$marker"
cp -- "$live" "$marker/kiln-identity"
chmod 755 "$marker/kiln-identity"
sync "$marker/kiln-identity"
atomic_copy "$package" "$live" || {
  restore_marker
  exit 1
}
if ! restart_identity_service || ! probe_ok; then
  restore_marker
  exit 1
fi
previous=$(retire_marker)
printf 'identity %s; previous %s\n' "$version" "$previous"
