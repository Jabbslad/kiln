#!/bin/bash
# Replace /usr/local/libexec/kiln-identity from the archive in DIRECTORY.
# The previous executable is kept under /opt/kiln/upgrades. A failed probe
# puts that executable back. This script does not install a unit file.
set -euo pipefail
umask 077

fail() { printf 'identity: %s\n' "$*" >&2; exit 1; }

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
  local i=0 body code
  while [ "$i" -lt 5 ]; do
    body=$(mktemp)
    code=$(curl --silent --show-error --proto '=http' --proto-redir '=http' --max-time 10 \
      --output "$body" --write-out '%{http_code}' \
      http://127.0.0.1:8091/login || true)
    if [ "$code" = 200 ] && grep -q 'Light the way in.' "$body"; then
      rm -f "$body"
      return 0
    fi
    rm -f "$body"
    i=$((i + 1))
    if [ "$i" -lt 5 ]; then
      sleep 1
    fi
  done
  return 1
}

public_ok() {
  local body code hdr
  body=$(mktemp)
  code=$(curl --silent --show-error --proto '=https' --proto-redir '=https' --max-time 10 \
    --output "$body" --write-out '%{http_code}' \
    https://dark-forge.dev/login || true)
  if [ "$code" != 200 ] || ! grep -q 'Light the way in.' "$body"; then
    rm -f "$body"
    return 1
  fi
  rm -f "$body"
  hdr=$(mktemp)
  code=$(curl --silent --show-error --proto '=https' --proto-redir '=https' --max-redirs 0 --max-time 10 \
    -D "$hdr" --output /dev/null --write-out '%{http_code}' \
    https://app.dark-forge.dev/login || true)
  if [ "$code" = 303 ] && grep -qi 'dark-forge.dev/oauth/server' "$hdr" && grep -q 'code_challenge_method=S256' "$hdr"; then
    rm -f "$hdr"
    return 0
  fi
  rm -f "$hdr"
  return 1
}

probe_ok() {
  loopback_ok && public_ok
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
  if loopback_ok; then
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

if cmp -s "$package" "$live"; then
  if ! process_matches; then
    systemctl restart kiln-identity.service
  fi
  probe_ok || fail 'identity probe failed'
  printf 'identity %s\n' "$version"
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
systemctl restart kiln-identity.service
if ! probe_ok; then
  restore_marker
  exit 1
fi
previous=$(retire_marker)
printf 'identity %s; previous %s\n' "$version" "$previous"
