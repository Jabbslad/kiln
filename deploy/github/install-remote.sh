#!/bin/bash
# Install or reuse one Kiln server release. Runs on the target as root.
set -euo pipefail
umask 077

fail() { printf 'deploy: %s\n' "$*" >&2; exit 1; }

dir=${1:?usage: install-remote.sh DIRECTORY [ADDRESS]}
address=${2:-}
case "$dir" in
  /tmp/kiln-deploy.[A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9][A-Za-z0-9]) ;;
  *) fail 'refusing to operate outside the deploy directory' ;;
esac
cd "$dir"

archive=$(printf '%s\n' kiln-server-*.tar.gz)
[ -f "$archive" ] || fail 'missing release archive'
[ -f "${archive}.sha256" ] || fail 'missing release checksum'
awk -v name="$archive" '
  NF != 2 || $2 != name || $1 !~ /^[0-9a-f]{64}$/ { bad = 1 }
  END { exit bad || NR != 1 }
' "${archive}.sha256" || fail 'unexpected checksum file'
sha256sum -c "${archive}.sha256"

extract=$(mktemp -d "$dir/extract.XXXXXX")
trap 'rm -rf "$extract"' EXIT
tar -tzf "$archive" > "$extract/names"
awk '
  /(^|\/)\.\.(\/|$)/ || /^\// || /\/$/ { bad = 1 }
  END { exit bad }
' "$extract/names" || fail 'archive path is not safe'
tar -tvzf "$archive" > "$extract/types"
awk 'substr($0, 1, 1) != "-" { bad = 1 } END { exit bad }' "$extract/types" \
  || fail 'archive contains a non-regular file'
mkdir "$extract/package"
tar -xzf "$archive" -C "$extract/package"
# v0.4.4 uploads this tree's installer. A newer release keeps the copy in the archive.
if [ -f "$dir/installer.sh" ]; then
  install -m 0755 "$dir/installer.sh" "$extract/package/install.sh"
fi
[ -f "$extract/package/install.sh" ] || fail 'archive is missing install.sh'

stamp() {
  if [ ! -d /run/systemd/system ]; then
    printf 'systemd-not-booted\n'
    return
  fi
  systemctl show kiln-identity.service \
    -p LoadState -p ActiveState -p ActiveEnterTimestamp -p NRestarts -p MainPID \
    --no-pager
}
restarts() {
  if [ ! -d /run/systemd/system ]; then
    printf 'unavailable\n'
    return
  fi
  systemctl show kiln-api.service -p NRestarts --value --no-pager
}
before=$(stamp)
printf 'kiln-api NRestarts before: %s\n' "$(restarts)"

existing=false
for path in /etc/kiln /opt/kiln /var/lib/kiln; do
  if [ -e "$path" ] || [ -L "$path" ]; then
    existing=true
  fi
done

if [ "$existing" = false ] && [ -z "$address" ]; then
  command -v ip >/dev/null 2>&1 || fail 'address detection requires iproute2'
  route=$(ip -4 route get 1.1.1.1) || fail 'cannot detect a route; pass a private IPv4 address'
  address=$(printf '%s\n' "$route" | awk '{
    for (i = 1; i < NF; i++) if ($i == "src") { value = $(i + 1); n++ }
    END { if (n == 1) print value }
  }')
  [ -n "$address" ] || fail 'cannot detect one IPv4 address; pass a private IPv4 address'
fi

if [ "$existing" = false ]; then
  # List-only mode. Install a package only when it is missing.
  export DEBIAN_FRONTEND=noninteractive
  export NEEDRESTART_MODE=l
  apt-get update
  apt-get install --no-install-recommends --no-upgrade -y \
    -o Dpkg::Options::=--force-confdef \
    -o Dpkg::Options::=--force-confold \
    openssl curl ca-certificates tar passwd
fi
if ! command -v sqlite3 >/dev/null 2>&1; then
  DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update
  DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3
fi

if [ -n "$address" ]; then
  printf '%s\n' "$address" | awk -F. '
    NF == 4 {
      for (i = 1; i <= 4; i++) if ($i !~ /^(0|[1-9][0-9]*)$/ || $i > 255) exit 1
      if ($1 == 10 || ($1 == 172 && $2 >= 16 && $2 <= 31) || ($1 == 192 && $2 == 168) ||
          ($1 == 100 && $2 >= 64 && $2 <= 127) || ($1 == 127 && $2 == 0 && $3 == 0 && $4 == 1)) ok = 1
    }
    END { exit !(ok && NR == 1) }
  ' || fail 'the API address must be a private or VPN IPv4 address, or 127.0.0.1'
fi

install_rc=0
if [ -n "$address" ]; then
  bash "$extract/package/install.sh" --address "$address" --apply || install_rc=$?
else
  bash "$extract/package/install.sh" --apply || install_rc=$?
fi

doctor_rc=1
if [ -x /opt/kiln/bin/kiln-runtime ]; then
  doctor_cmd=(/opt/kiln/bin/kiln-runtime)
  if [ -f /etc/kiln/isolation.json ]; then
    doctor_cmd+=(--state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json)
  fi
  doctor_cmd+=(doctor)
  set +e
  report=$(PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin "${doctor_cmd[@]}" 2>"$extract/doctor.err")
  doctor_status=$?
  set -e
  printf 'kiln-runtime doctor:\n%s\n' "$report"
  if [ -s "$extract/doctor.err" ]; then
    cat "$extract/doctor.err" >&2
  fi
  doctor_rc=$doctor_status
  flat=$(printf '%s' "$report" | tr -d '[:space:]')
  case $flat in
    *'"development_ready":true'*) ;;
    *) doctor_rc=1 ;;
  esac
fi
if [ "$doctor_rc" -ne 0 ]; then
  printf 'deploy: kiln-runtime doctor did not report development_ready true\n' >&2
fi

health_rc=1
if [ "$install_rc" -eq 0 ]; then
  if [ -z "$address" ]; then
    address=$(awk '
      match($0, /--listen [0-9.]+:8443/) {
        value = substr($0, RSTART, RLENGTH)
        sub(/^--listen /, "", value)
        sub(/:8443$/, "", value)
        print value
        found = 1
      }
      END { exit !found }
    ' /etc/systemd/system/kiln-api.service.d/installer.conf) || fail 'cannot read the installed API address'
  fi
  health_rc=0
  bash "$extract/package/install.sh" --healthcheck "$address" || health_rc=$?
else
  printf 'deploy: skipping the health check because installation failed\n' >&2
fi

smoke_rc=1
if [ "$install_rc" -eq 0 ] && [ "$doctor_rc" -eq 0 ] && [ "$health_rc" -eq 0 ]; then
  smoke_rc=0
  bash "$dir/smoke.sh" "$address" || smoke_rc=$?
else
  printf 'deploy: skipping the guest smoke test because installation failed\n' >&2
fi

printf 'kiln-api NRestarts after: %s\n' "$(restarts)"
after=$(stamp)
if [ "$before" != "$after" ]; then
  printf 'deploy: kiln-identity changed during deploy\n' >&2
  printf 'before:\n%s\nafter:\n%s\n' "$before" "$after" >&2
  exit 1
fi
[ "$install_rc" -eq 0 ] && [ "$doctor_rc" -eq 0 ] && [ "$health_rc" -eq 0 ] && [ "$smoke_rc" -eq 0 ]
