#!/bin/bash
# Install or update a verified Kiln server release, preserving existing state.
set -euo pipefail
umask 077
export LC_ALL=C

die() {
  printf 'Setup stopped: %s\nRetain state and inspect logs; do not delete VM state or blindly retry an interrupted update.\n' "$1" >&2
  exit 1
}

if [ -n "${KILN_INSTALL_FIXTURE:-}" ]; then
  root=$KILN_INSTALL_FIXTURE
  ETC=$root/etc/kiln
  STATE=$root/var/lib/kiln
  OPT=$root/opt/kiln
  LIBEXEC=$root/usr/local/libexec
  UNITS=$root/etc/systemd/system
  CGROUP=$root/sys/fs/cgroup/kiln
  LOCK=$root/run/lock/kiln-install.lock
  RUN_KILN=$root/run/kiln
  OS_RELEASE=$root/etc/os-release
  MEMINFO=$root/proc/meminfo
  CONTROLLERS=$root/sys/fs/cgroup/cgroup.controllers
  SYSTEMD_RUN=$root/run/systemd/system
  KVM_NODE=$root/dev/kvm
  SUBUID=$root/etc/subuid
  SUBGID=$root/etc/subgid
  PASSWD=$root/etc/passwd
  GROUP=$root/etc/group
  PROC_ROOT=$root/proc
else
  ETC=/etc/kiln
  STATE=/var/lib/kiln
  OPT=/opt/kiln
  LIBEXEC=/usr/local/libexec
  UNITS=/etc/systemd/system
  CGROUP=/sys/fs/cgroup/kiln
  LOCK=/run/lock/kiln-install.lock
  RUN_KILN=/run/kiln
  OS_RELEASE=/etc/os-release
  MEMINFO=/proc/meminfo
  CONTROLLERS=/sys/fs/cgroup/cgroup.controllers
  SYSTEMD_RUN=/run/systemd/system
  KVM_NODE=/dev/kvm
  SUBUID=/etc/subuid
  SUBGID=/etc/subgid
  PASSWD=
  GROUP=
  PROC_ROOT=/proc
  PATH=/usr/sbin:/usr/bin:/sbin:/bin
  export PATH
fi

# Doctor finds Firecracker only when this directory is first. The copies are not
# on the default PATH.
DOCTOR_PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin
atomic_tmp=
HEALTH_ERROR=
host_stopped=0

cleanup() {
  status=$?
  if [ -n "${atomic_tmp:-}" ]; then
    rm -f -- "$atomic_tmp"
  fi
  exit "$status"
}
trap cleanup EXIT

binary_names() {
  printf '%s\n' kiln kiln-runtime kiln-network kiln-host kiln-api
}

dest_for() {
  case $1 in
    kiln|kiln-runtime|kiln-network) printf '%s\n' "$OPT/bin/$1" ;;
    kiln-host|kiln-api) printf '%s\n' "$LIBEXEC/$1" ;;
    *) die "internal: unknown binary $1" ;;
  esac
}

reported_name() {
  case $1 in
    kiln-host|kiln-api) printf '%s\n' kiln-server ;;
    *) printf '%s\n' "$1" ;;
  esac
}

mode_is_publicly_writable() {
  local mode=$1 group other
  group=${mode: -2:1}
  other=${mode: -1}
  case $group in 2|3|6|7) return 0 ;; esac
  case $other in 2|3|6|7) return 0 ;; esac
  return 1
}

has_exec_bit() {
  local mode=$1 rest digit
  rest=$mode
  while [ -n "$rest" ]; do
    digit=${rest:0:1}
    rest=${rest:1}
    case $digit in 1|3|5|7) return 0 ;; esac
  done
  return 1
}

json_string() {
  local file=$1 key=$2
  awk -v key="$key" '
    BEGIN { RS = "" }
    {
      s = $0
      pat = "\"" key "\""
      p = index(s, pat)
      if (p == 0) exit 1
      rest = substr(s, p + length(pat))
      if (!match(rest, /^[[:space:]]*:[[:space:]]*"/)) exit 1
      rest = substr(rest, RLENGTH + 1)
      q = index(rest, "\"")
      if (q == 0) exit 1
      printf "%s", substr(rest, 1, q - 1)
    }
  ' "$file"
}

json_has_value() {
  local file=$1 key=$2
  awk -v key="$key" '
    BEGIN { RS = "" }
    {
      s = $0
      pat = "\"" key "\""
      p = index(s, pat)
      if (p == 0) exit 1
      rest = substr(s, p + length(pat))
      sub(/^[[:space:]]*:[[:space:]]*/, "", rest)
      c = substr(rest, 1, 1)
      if (c == "{" || c == "[" || c == "t") exit 0
      exit 1
    }
  ' "$file"
}

is_version() {
  printf '%s\n' "$1" | awk '
    BEGIN { FS = "." }
    NF == 3 && $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ { ok = 1 }
    END { exit !ok }
  '
}

# Exit 0 when equal, 1 when the first is older, 2 when the first is newer.
# Components are integers, so 0.4.10 is newer than 0.4.4.
version_cmp() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    split(a, x, ".")
    split(b, y, ".")
    for (i = 1; i <= 3; i++) {
      if (x[i] + 0 < y[i] + 0) exit 1
      if (x[i] + 0 > y[i] + 0) exit 2
    }
    exit 0
  }'
}

private_address() {
  local value=$1
  printf '%s\n' "$value" | awk -F. '
    NF == 4 {
      for (i = 1; i <= 4; i++) if ($i !~ /^(0|[1-9][0-9]*)$/ || $i > 255) exit 1
      if ($1 == 10 || ($1 == 172 && $2 >= 16 && $2 <= 31) || ($1 == 192 && $2 == 168) ||
          ($1 == 100 && $2 >= 64 && $2 <= 127) || ($1 == 127 && $2 == 0 && $3 == 0 && $4 == 1)) ok = 1
    }
    END { exit !(ok && NR == 1) }
  ' || die "use a private/VPN IPv4 address, or 127.0.0.1 for local-only access"
  printf '%s\n' "$value"
}

require_absent() {
  local path
  for path in "$@"; do
    if [ -e "$path" ] || [ -L "$path" ]; then
      die "refusing existing destination $path; this is not an upgrade/recovery tool"
    fi
  done
}

check_subids() {
  local file=$1 base=$2 err
  [ -f "$file" ] || return 0
  if ! err=$(awk -v base="$base" '
    /^[[:space:]]*($|#)/ { next }
    {
      n = split($0, p, ":")
      if (n < 3) { print "unsupported subordinate id line"; exit 1 }
      first = p[n - 1] + 0
      count = p[n] + 0
      if (first < base + 8 && first + count > base) {
        printf "reserved range %d..%d overlaps subordinate IDs\n", base, base + 7
        exit 1
      }
    }
  ' "$file"); then
    die "$err"
  fi
}

trusted_ancestors() {
  local path=$1 parent uid mode
  parent=$path
  while true; do
    if [ -L "$parent" ]; then
      die "symlink destination ancestor: $parent"
    fi
    if [ -e "$parent" ]; then
      if [ ! -d "$parent" ]; then
        die "destination ancestor must be a root-owned, non-writable directory: $parent"
      fi
      if [ -z "${KILN_INSTALL_FIXTURE:-}" ]; then
        uid=$(stat -c '%u' "$parent")
        mode=$(stat -c '%a' "$parent")
        if [ "$uid" != 0 ] || mode_is_publicly_writable "$mode"; then
          die "destination ancestor must be a root-owned, non-writable directory: $parent"
        fi
      fi
    fi
    [ "$parent" = / ] && break
    parent=$(dirname "$parent")
  done
}

accounts_text() {
  local kind=$1
  if [ -n "${KILN_INSTALL_FIXTURE:-}" ]; then
    if [ "$kind" = passwd ]; then
      cat "$PASSWD"
    else
      cat "$GROUP"
    fi
    return 0
  fi
  if [ "$kind" = passwd ]; then
    getent passwd
  else
    getent group
  fi
}

bind_port() {
  perl -MSocket -e '
    use strict;
    my $addr = inet_aton($ARGV[0]) or die "bad address\n";
    socket(my $sock, PF_INET, SOCK_STREAM, 0) or die "socket: $!\n";
    bind($sock, pack_sockaddr_in(8443, $addr)) or die "bind: $!\n";
  ' "$1" >/dev/null 2>&1 || die "the server address must be assigned on this host and port 8443 must be free"
}

have_tool() {
  if [ -n "${KILN_INSTALL_MISSING:-}" ]; then
    case " ${KILN_INSTALL_MISSING} " in
      *" $1 "*) return 1 ;;
    esac
  fi
  command -v "$1" >/dev/null 2>&1
}

preflight() {
  local bundle=$1 address=$2 tool parent service
  local ID="" VERSION_ID=""
  address=$(private_address "$address")
  [ -r "$OS_RELEASE" ] || die "installer supports Ubuntu 24.04, Ubuntu 26.04, and Debian 13 x86-64 only"
  # shellcheck disable=SC1090
  . "$OS_RELEASE"
  case "${ID:-}:${VERSION_ID:-}" in
    ubuntu:24.04|ubuntu:26.04|debian:13) ;;
    *) die "installer supports Ubuntu 24.04, Ubuntu 26.04, and Debian 13 x86-64 only" ;;
  esac
  [ "$(uname -m)" = x86_64 ] || die "installer supports Ubuntu 24.04, Ubuntu 26.04, and Debian 13 x86-64 only"
  [ -d "$SYSTEMD_RUN" ] || die "a booted systemd host is required (not a container)"
  for tool in openssl systemctl systemd-analyze groupadd useradd curl bash tar sha256sum install realpath sqlite3 perl flock sync; do
    have_tool "$tool" || die "missing prerequisite: $tool; install host packages before retrying"
  done
  if ! ( : <>"$KVM_NODE" ) 2>/dev/null; then
    die "cannot open /dev/kvm for read and write"
  fi
  awk '
    { for (i = 1; i <= NF; i++) seen[$i] = 1 }
    END { exit !(seen["cpu"] && seen["memory"] && seen["pids"]) }
  ' "$CONTROLLERS" || die "cgroup v2 with cpu, memory and pids controllers is required"
  require_absent \
    "$ETC" "$STATE" "$OPT" "$CGROUP" "$RUN_KILN" \
    "$LIBEXEC/kiln-host" "$LIBEXEC/kiln-api" \
    "$UNITS/kiln-cgroup.service" "$UNITS/kiln-host.service" "$UNITS/kiln-api.service" \
    "$UNITS/kiln-host.service.d" "$UNITS/kiln-api.service.d"
  for parent in "$ETC" "$STATE" "$OPT" "$CGROUP" "$RUN_KILN" \
    "$LIBEXEC/kiln-host" "$LIBEXEC/kiln-api" \
    "$UNITS/kiln-cgroup.service" "$UNITS/kiln-host.service" "$UNITS/kiln-api.service" \
    "$UNITS/kiln-host.service.d" "$UNITS/kiln-api.service.d"; do
    trusted_ancestors "$(dirname "$parent")"
  done
  parent=$LIBEXEC
  while true; do
    if [ -e "$parent" ]; then
      [ -x "$parent" ] || die "the unprivileged gateway needs directory traversal through $parent"
    fi
    [ "$parent" = / ] && break
    parent=$(dirname "$parent")
  done
  for service in kiln-cgroup.service kiln-host.service kiln-api.service; do
    if [ "$(systemctl show --property=LoadState --value "$service" 2>/dev/null | tr -d '[:space:]')" != not-found ]; then
      die "an existing $service unit is installed"
    fi
  done
  if accounts_text passwd | awk -F: '
    BEGIN { split("kiln-api kiln-vm0 kiln-vm1 kiln-vm2 kiln-vm3 kiln-vm4 kiln-vm5 kiln-vm6 kiln-vm7", names, " "); for (i in names) bad[names[i]] = 1 }
    bad[$1] || ($3 >= 70000 && $3 <= 70007) { found = 1 }
    END { exit found ? 0 : 1 }
  '; then
    die "kiln account name or UID range 70000..70007 is already allocated"
  fi
  if accounts_text group | awk -F: '
    BEGIN { split("kiln-api kiln-vm0 kiln-vm1 kiln-vm2 kiln-vm3 kiln-vm4 kiln-vm5 kiln-vm6 kiln-vm7", names, " "); for (i in names) bad[names[i]] = 1 }
    bad[$1] || ($3 >= 71000 && $3 <= 71007) { found = 1 }
    END { exit found ? 0 : 1 }
  '; then
    die "kiln group name or GID range 71000..71007 is already allocated"
  fi
  check_subids "$SUBUID" 70000
  check_subids "$SUBGID" 71000
  if [ -d "$PROC_ROOT" ]; then
    local status
    shopt -s nullglob
    for status in "$PROC_ROOT"/[0-9]*/status; do
      if [ -f "$status" ] && ! awk '
        /^Uid:/ { base = 70000 }
        /^Gid:/ { base = 71000 }
        /^Uid:/ || /^Gid:/ {
          for (i = 2; i <= NF; i++) if ($i + 0 >= base && $i + 0 < base + 8) found = 1
        }
        END { exit found ? 1 : 0 }
      ' "$status"; then
        if [ -e "$status" ]; then
          shopt -u nullglob
          die "a running process uses the intended jail identity range"
        fi
      fi
    done
    shopt -u nullglob
  fi
  local avail parent_dir
  parent_dir=$(dirname "$STATE")
  avail=$(df -B1 "$parent_dir" | awk 'NR == 2 { print $4 }')
  [ "${avail:-0}" -ge $((24 * 1024 * 1024 * 1024)) ] || die "at least 24 GiB free on /var/lib is required for initial copy-backend storage"
  awk '/^MemAvailable:/ { kb = $2 } END { exit !(kb + 0 >= 6 * 1024 * 1024) }' "$MEMINFO" \
    || die "at least 6 GiB available host RAM is required for the initial 4 GiB template"
  bind_port "$address"
  local relative path manifest boot arch
  for relative in \
    bin/kiln-runtime bin/kiln bin/kiln-host bin/kiln-api bin/kiln-network \
    fetch-firecracker.sh deploy/kiln-host.service deploy/kiln-api.service \
    image/image.json image/inputs.json; do
    path=$bundle/$relative
    if [ -L "$path" ] || [ ! -f "$path" ]; then
      die "missing regular release input: $relative"
    fi
  done
  manifest=$bundle/image/image.json
  boot=$(json_string "$manifest" boot_mode || true)
  arch=$(json_string "$manifest" architecture || true)
  if [ "$boot" != systemd_warm ] || [ "$arch" != x86_64 ]; then
    die "release must contain the per-box-identity warm x86-64 image"
  fi
  local kind name expected digest actual
  for kind in kernel rootfs; do
    if [ "$kind" = kernel ]; then name=vmlinux; else name=rootfs.ext4; fi
    path=$bundle/image/$name
    expected=$(json_string "$manifest" "${kind}_path" || true)
    digest=$(json_string "$manifest" "${kind}_sha256" || true)
    if [ "$expected" != "$name" ] || [ -L "$path" ] || [ ! -f "$path" ]; then
      die "invalid release image path: $name"
    fi
    actual=$(sha256sum "$path" | awk '{ print $1 }')
    [ "$actual" = "$digest" ] || die "release image checksum mismatch: $name"
  done
}

write_file() {
  local path=$1 mode=$2
  mkdir -p -- "$(dirname "$path")"
  (set -C; cat >"$path")
  chmod "$mode" "$path"
}

network_preflight() {
  local uplink=$1 tool net
  printf '%s\n' "$uplink" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,14}$' \
    || die "network uplink must be a Linux interface name"
  for tool in ip nft sysctl nsenter; do
    have_tool "$tool" || die "networking requires iproute2, nftables and util-linux packages"
  done
  ip link show dev "$uplink" >/dev/null 2>&1 || die "network uplink must be a Linux interface name"
  if [ -n "${KILN_INSTALL_FIXTURE:-}" ]; then
    net=$root/sys/class/net/kiln0
  else
    net=/sys/class/net/kiln0
  fi
  require_absent "$net" "$UNITS/kiln-network.service"
}

write_config() {
  local network=$1
  if [ "$network" = 1 ]; then
    write_file "$ETC/isolation.json" 600 <<'EOF'
{
  "firecracker": "/opt/kiln/bin/firecracker",
  "jailer": "/opt/kiln/bin/jailer",
  "cgroup_parent": "kiln",
  "uid_base": 70000,
  "gid_base": 71000,
  "disk_backend": "copy",
  "network": {
    "helper": "/opt/kiln/bin/kiln-network",
    "namespace_scope": "kiln",
    "resolver": "1.1.1.1"
  }
}
EOF
  else
    write_file "$ETC/isolation.json" 600 <<'EOF'
{
  "firecracker": "/opt/kiln/bin/firecracker",
  "jailer": "/opt/kiln/bin/jailer",
  "cgroup_parent": "kiln",
  "uid_base": 70000,
  "gid_base": 71000,
  "disk_backend": "copy"
}
EOF
  fi
  write_host ""
}

write_host() {
  local template_id=$1
  if [ -n "$template_id" ]; then
    # The empty template file was created before the build. This is the one rewrite.
    rm -f -- "$ETC/host.json"
    write_file "$ETC/host.json" 600 <<EOF
{
  "runtime_dir": "/var/lib/kiln/runtime",
  "journal_dir": "/var/lib/kiln/journal",
  "socket": "/run/kiln/host.sock",
  "isolation_config": "/etc/kiln/isolation.json",
  "allow_unsafe_development": false,
  "templates": {
    "ubuntu-4g": "$template_id"
  }
}
EOF
  else
    write_file "$ETC/host.json" 600 <<'EOF'
{
  "runtime_dir": "/var/lib/kiln/runtime",
  "journal_dir": "/var/lib/kiln/journal",
  "socket": "/run/kiln/host.sock",
  "isolation_config": "/etc/kiln/isolation.json",
  "allow_unsafe_development": false,
  "templates": {}
}
EOF
  fi
}

account_ids() {
  local name=$1
  if [ -n "${KILN_INSTALL_FIXTURE:-}" ]; then
    awk -F: -v n="$name" '$1 == n { print $3, $4; found = 1 } END { exit !found }' "$PASSWD"
  else
    printf '%s %s\n' "$(id -u "$name")" "$(id -g "$name")"
  fi
}

place_file() {
  local source=$1 dest=$2 mode=$3
  if [ "$(id -u)" -eq 0 ]; then
    install -o root -g root -m "$mode" "$source" "$dest"
  else
    install -m "$mode" "$source" "$dest"
  fi
}

credentials() {
  local uid=$1 gid=$2 address=$3 dir serial
  dir=$ETC
  private_address "$address" >/dev/null
  openssl req -x509 -newkey rsa:3072 -nodes -days 3650 \
    -subj "/CN=kiln private CA" \
    -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -keyout "$dir/ca.key" -out "$dir/ca.crt" >/dev/null 2>&1
  openssl req -new -newkey rsa:2048 -nodes \
    -subj "/CN=$address" \
    -keyout "$dir/tls.key" -out "$dir/tls.csr" >/dev/null 2>&1
  write_file "$dir/tls.ext" 600 <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=IP:$address
EOF
  serial=0x$(openssl rand -hex 16)
  if [ "$serial" = 0x00000000000000000000000000000000 ]; then
    serial=1
  fi
  openssl x509 -req -in "$dir/tls.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
    -set_serial "$serial" -days 365 -sha256 -extfile "$dir/tls.ext" \
    -out "$dir/tls.crt" >/dev/null 2>&1
  openssl rand -hex 32 | awk '{ printf "%s\n", $0 }' >"$dir/admin.token"
  chmod 600 "$dir/ca.key" "$dir/tls.key" "$dir/admin.token"
  if [ "$(id -u)" -eq 0 ]; then
    chown "$uid:$gid" "$dir/tls.key" "$dir/tls.crt" "$dir/admin.token"
  fi
  chmod 644 "$dir/ca.crt" "$dir/tls.crt"
  rm -f -- "$dir/tls.csr" "$dir/tls.ext"
  write_file "$dir/CONNECT.txt" 600 <<EOF
This bundle grants administrator access. Store it privately; never upload it to GitHub.
Run from this extracted directory on your laptop with kiln on PATH:

kiln profile add default --url https://$address:8443 --token-file admin.token --ca-file ca.crt
kiln templates
kiln create --template ubuntu-4g --name first-box

Keep this directory: the profile stores file references, not copies.
Unix: chmod 700 .; chmod 600 admin.token
Windows: restrict this directory and token's ACL to your own account.
The server leaf certificate expires in one year; arrange renewal before expiry.
EOF
  tar --create --gzip --file "$dir/laptop.tar.gz" \
    --owner=0 --group=0 --mode=0600 \
    --transform 's,^,kiln-connection/,' \
    -C "$dir" admin.token ca.crt CONNECT.txt
  chmod 600 "$dir/laptop.tar.gz"
}

write_units() {
  local address=$1
  private_address "$address" >/dev/null
  write_file "$UNITS/kiln-cgroup.service" 644 <<'EOF'
[Unit]
Description=Prepare kiln cgroup v2 controllers
After=local-fs.target
Before=kiln-host.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -ec 'printf "+cpu +memory +pids\n" > /sys/fs/cgroup/cgroup.subtree_control; mkdir -p /sys/fs/cgroup/kiln; printf "+cpu +memory +pids\n" > /sys/fs/cgroup/kiln/cgroup.subtree_control'

[Install]
WantedBy=multi-user.target
EOF
  mkdir -p -- "$UNITS/kiln-host.service.d" "$UNITS/kiln-api.service.d"
  write_file "$UNITS/kiln-host.service.d/installer.conf" 644 <<'EOF'
[Unit]
Requires=kiln-cgroup.service
After=kiln-cgroup.service
EOF
  write_file "$UNITS/kiln-api.service.d/installer.conf" 644 <<EOF
[Service]
ExecStart=
ExecStart=/usr/local/libexec/kiln-api --listen $address:8443 --host-socket /run/kiln/host.sock --token-file /etc/kiln/admin.token --tls-cert /etc/kiln/tls.crt --tls-key /etc/kiln/tls.key
EOF
}

healthcheck() {
  local address=$1 expected=${2:-}
  local i=0 body norm exp
  HEALTH_ERROR="HTTPS readiness failed; inspect journalctl -u kiln-host -u kiln-api"
  while [ "$i" -lt 30 ]; do
    # -H @- reads the header from stdin. The token must not be a curl argument:
    # any local user can read another process's argv from /proc.
    if body=$(
      awk 'NR == 1 { printf "Authorization: Bearer %s\n", $0 }' "$ETC/admin.token" |
        curl --silent --show-error --cacert "$ETC/ca.crt" --noproxy '*' --max-time 2 \
          -H @- \
          "https://${address}:8443/v1/templates" 2>/dev/null
    ); then
      norm=$(printf '%s' "$body" | tr -d '[:space:]')
      case $norm in
        \[*\]) ;;
        *)
          HEALTH_ERROR="installed service returned an unexpected template catalog"
          return 1
          ;;
      esac
      if [ -n "$expected" ]; then
        exp=$(printf '%s' "$expected" | tr -d '[:space:]')
        if [ "$norm" != "$exp" ]; then
          HEALTH_ERROR="installed service returned an unexpected template catalog"
          return 1
        fi
      fi
      printf '%s' "$norm"
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done
  return 1
}

disable_fresh() {
  local service
  for service in "$@"; do
    systemctl disable --now "$service" >/dev/null 2>&1 || true
  done
}

install_fresh() {
  local bundle=$1 address=$2 uplink=${3:-}
  local name slot uid gid template_id services runtime ids
  address=$(private_address "$address")
  mkdir -m 755 "$ETC"
  chmod 755 "$ETC"
  mkdir -m 700 "$STATE"
  mkdir -m 755 "$OPT"
  chmod 755 "$OPT"
  mkdir -m 755 "$OPT/bin"
  chmod 755 "$OPT/bin"
  if [ ! -d "$LIBEXEC" ]; then
    mkdir -m 755 "$LIBEXEC"
    chmod 755 "$LIBEXEC"
  fi
  for name in kiln-runtime kiln kiln-network; do
    place_file "$bundle/bin/$name" "$OPT/bin/$name" 0755
  done
  for name in kiln-host kiln-api; do
    place_file "$bundle/bin/$name" "$LIBEXEC/$name" 0755
  done
  printf 'Downloading checksum-pinned Firecracker 1.17.0...\n'
  bash "$bundle/fetch-firecracker.sh" "$OPT/bin"
  groupadd --system kiln-api
  useradd --system --gid kiln-api --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin kiln-api
  for slot in 0 1 2 3 4 5 6 7; do
    name="kiln-vm${slot}"
    groupadd --gid "$((71000 + slot))" "$name"
    useradd --uid "$((70000 + slot))" --gid "$name" --no-create-home --no-log-init \
      --home-dir /nonexistent --shell /usr/sbin/nologin --password '!' "$name"
  done
  ids=$(account_ids kiln-api) || die "kiln-api account was not created"
  read -r uid gid <<<"$ids"
  if [ -z "${uid:-}" ] || [ -z "${gid:-}" ]; then
    die "kiln-api account was not created"
  fi
  credentials "$uid" "$gid" "$address"
  mkdir -m 700 "$STATE/image"
  for name in vmlinux rootfs.ext4 image.json inputs.json; do
    place_file "$bundle/image/$name" "$STATE/image/$name" 0444
  done
  if [ -n "$uplink" ]; then
    write_config 1
  else
    write_config 0
  fi
  services=(kiln-cgroup.service kiln-host.service kiln-api.service)
  if [ -n "$uplink" ]; then
    write_file "$UNITS/kiln-network.service" 644 <<EOF
[Unit]
Description=kiln internet egress policy
Wants=network-online.target
After=network-online.target
Before=kiln-host.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/opt/kiln/bin/kiln-network provision $uplink

[Install]
WantedBy=multi-user.target
EOF
    services=(kiln-network.service kiln-cgroup.service kiln-host.service kiln-api.service)
  fi
  place_file "$bundle/deploy/kiln-host.service" "$UNITS/kiln-host.service" 0644
  place_file "$bundle/deploy/kiln-api.service" "$UNITS/kiln-api.service" 0644
  write_units "$address"
  if [ -n "$uplink" ]; then
    write_file "$UNITS/kiln-host.service.d/network.conf" 644 <<'EOF'
[Unit]
Requires=kiln-network.service
After=kiln-network.service
EOF
  fi
  systemd-analyze verify "${services[@]/#/$UNITS/}" >/dev/null 2>&1
  systemctl daemon-reload
  if [ -n "$uplink" ]; then
    systemctl start kiln-network.service
  fi
  systemctl start kiln-cgroup.service
  if [ -n "${KILN_INSTALL_FIXTURE:-}" ]; then
    runtime="$OPT/bin:$PATH"
  else
    runtime=$DOCTOR_PATH
  fi
  env "PATH=$runtime" "$OPT/bin/kiln-runtime" \
    --state-dir "$STATE/runtime" --isolation-config "$ETC/isolation.json" doctor >/dev/null
  printf 'Preparing the host-specific 4 GiB warm template...\n'
  template_id=$(env "PATH=$runtime" "$OPT/bin/kiln-runtime" \
    --state-dir "$STATE/runtime" --isolation-config "$ETC/isolation.json" \
    template build --image "$STATE/image/image.json" --memory-mib 4096 --vcpus 1 --profile isolated)
  template_id=$(printf '%s\n' "$template_id" | awk '
    match($0, /"id"[[:space:]]*:[[:space:]]*"[a-f0-9]{32}"/) {
      s = substr($0, RSTART, RLENGTH)
      sub(/.*"id"[[:space:]]*:[[:space:]]*"/, "", s)
      sub(/".*/, "", s)
      print s
      found = 1
    }
    END { exit !found }
  ') || die "template build returned an invalid ID"
  printf '%s\n' "$template_id" | grep -Eq '^[a-f0-9]{32}$' || die "template build returned an invalid ID"
  write_host "$template_id"
  "$LIBEXEC/kiln-host" --config "$ETC/host.json" --check >/dev/null 2>&1
  if ! systemctl enable --now "${services[@]}" >/dev/null 2>&1; then
    if [ -n "$uplink" ]; then
      disable_fresh kiln-api.service kiln-host.service kiln-cgroup.service kiln-network.service
    else
      disable_fresh kiln-api.service kiln-host.service kiln-cgroup.service
    fi
    die "command failed: systemctl enable"
  fi
  if ! healthcheck "$address" '[{"name":"ubuntu-4g","memory_mib":4096,"vcpus":1}]' >/dev/null; then
    if [ -n "$uplink" ]; then
      disable_fresh kiln-api.service kiln-host.service kiln-cgroup.service kiln-network.service
    else
      disable_fresh kiln-api.service kiln-host.service kiln-cgroup.service
    fi
    die "$HEALTH_ERROR"
  fi
  printf 'Ready: https://%s:8443\nKeep /etc/kiln/laptop.tar.gz private for direct administrator recovery.\nTLS renewal is due within one year.\n' "$address"
  connection_instructions "$address"
}

connection_instructions() {
  local address=$1
  if [ -e "$ETC/enrollment.pending.json" ]; then
    printf 'Resume server enrollment: sudo %s/kiln-api enroll --resume\n' "$LIBEXEC"
  elif [ -e "$ETC/identity.json" ]; then
    printf 'Server enrollment preserved. On your laptop, run: kiln login\n'
  else
    printf 'One-time browser enrollment (does not restart guest VMs):\n'
    printf '  sudo %s/kiln-api enroll --url https://%s:8443\n' "$LIBEXEC" "$address"
    printf 'Approve using GitHub or Google, then run kiln login on your laptop using the same provider.\n'
    printf 'Installation never enrolls a server or opens a browser automatically.\n'
  fi
  if [ "$address" = "127.0.0.1" ]; then
    printf 'That URL is only on this machine. For dark-forge, follow docs/runbooks/2026-10-19-public-door.md and run deploy/enroll-dark-forge.sh.\n'
  fi
}

acquire_lock() {
  local uid mode inode_fd inode_path
  mkdir -p -- "$(dirname "$LOCK")"
  if [ -L "$LOCK" ]; then
    die "unsafe installer lock"
  fi
  : >>"$LOCK"
  exec {LOCK_FD}<>"$LOCK"
  # stat on /proc/self/fd does not follow that symlink; -L reads the open file.
  if [ ! -f "/proc/self/fd/${LOCK_FD}" ]; then
    die "unsafe installer lock"
  fi
  uid=$(stat -L -c '%u' "/proc/self/fd/${LOCK_FD}")
  mode=$(stat -L -c '%a' "/proc/self/fd/${LOCK_FD}")
  inode_fd=$(stat -L -c '%i' "/proc/self/fd/${LOCK_FD}")
  inode_path=$(stat -L -c '%i' "$LOCK")
  if [ -L "$LOCK" ] || [ "$uid" != "$(id -u)" ] || [ "$inode_fd" != "$inode_path" ]; then
    die "unsafe installer lock"
  fi
  if mode_is_publicly_writable "$mode"; then
    die "unsafe installer lock"
  fi
  # Unlinking the lock would free the inode and let a second installer take a new one.
  flock -n "$LOCK_FD" || die "another installer is running"
}

atomic_copy() {
  local source=$1 destination=$2 dir tmp
  dir=$(dirname "$destination")
  tmp=$(mktemp "$dir/.kiln-update-XXXXXX")
  atomic_tmp=$tmp
  if ! cp -- "$source" "$tmp"; then
    rm -f -- "$tmp"
    atomic_tmp=
    return 1
  fi
  # Callers invoke this under `if !`, which disables set -e for the whole function.
  # A failing mv would otherwise be hidden by the status of the final sync.
  chmod 755 "$tmp" || { rm -f -- "$tmp"; atomic_tmp=; return 1; }
  sync "$tmp" || { rm -f -- "$tmp"; atomic_tmp=; return 1; }
  mv -f -- "$tmp" "$destination" || { rm -f -- "$tmp"; atomic_tmp=; return 1; }
  atomic_tmp=
  sync "$dir" || return 1
}

regular_file() {
  local path=$1 executable=${2:-0} uid mode
  if [ -L "$path" ] || [ ! -f "$path" ]; then
    REFUSE="missing regular file: $path; partial installs require recovery"
    return 1
  fi
  if [ "$executable" = 1 ]; then
    uid=$(stat -c '%u' "$path")
    mode=$(stat -c '%a' "$path")
    if [ "$uid" != "$(id -u)" ] || mode_is_publicly_writable "$mode" || ! has_exec_bit "$mode"; then
      REFUSE="unsafe executable: $path"
      return 1
    fi
  fi
}

need_file() {
  regular_file "$@" || die "$REFUSE"
}

service_property() {
  local service=$1 name=$2
  systemctl show "--property=${name}" --value "$service" 2>/dev/null | tr -d '\r'
}

service_argv() {
  local service=$1 executable=$2 value path argv
  value=$(service_property "$service" ExecStart)
  case $value in
    "{ path="*" ; argv[]="*" ; "*"}") ;;
    *) die "unsupported ExecStart for $service" ;;
  esac
  path=${value#"{ path="}
  path=${path%%" ; argv[]="*}
  argv=${value#*" ; argv[]="}
  argv=${argv%%" ; "*}
  [ "$path" = "$executable" ] || die "unsupported ExecStart for $service"
  printf '%s\n' "$argv"
}

wait_for_operations() {
  local journal=$1 count n
  regular_file "$journal" || return 1
  have_tool sqlite3 || return 1
  count=$(sqlite3 -readonly "$journal" 'PRAGMA user_version;' 2>/dev/null) || return 1
  [ "$count" = 1 ] || return 1
  n=0
  while [ "$n" -lt 60 ]; do
    count=$(sqlite3 -readonly "$journal" 'SELECT count(*) FROM operations WHERE running=1;' 2>/dev/null) || return 1
    [ "$count" = 0 ] && return 0
    n=$((n + 1))
    sleep 1
  done
  return 1
}

read_version() {
  local path=$1 name=$2 output
  output=$("$path" --version 2>/dev/null || true)
  printf '%s\n' "$output" | awk -v name="$name" '
    NR == 1 && $0 ~ "^" name " [0-9]+\\.[0-9]+\\.[0-9]+$" { print $2; good = 1 }
    END { exit !(NR == 1 && good) }
  '
}

upgrade_swap() {
  local catalog=$1 address=$2 name
  systemctl stop kiln-api.service || return 1
  wait_for_operations "$STATE/journal/operations.sqlite" || return 1
  host_stopped=1
  systemctl stop kiln-host.service || return 1
  while IFS= read -r name; do
    atomic_copy "$bundle/bin/$name" "$(dest_for "$name")" || return 1
  done < <(binary_names)
  systemctl start kiln-host.service || return 1
  systemctl start kiln-api.service || return 1
  healthcheck "$address" "$catalog" >/dev/null || return 1
}

upgrade_restore() {
  local catalog=$1 address=$2 name
  if [ "$host_stopped" = 1 ]; then
    systemctl stop kiln-api.service || return 1
    systemctl stop kiln-host.service || return 1
    while IFS= read -r name; do
      atomic_copy "$backup/$name" "$(dest_for "$name")" || return 1
    done < <(binary_names)
    systemctl start kiln-host.service || return 1
  fi
  systemctl start kiln-api.service || return 1
  healthcheck "$address" "$catalog" >/dev/null || return 1
}

close_public_door() {
  local unit state changed=0 before
  unit=$UNITS/kiln-door-proxy.service
  if [ -f "$unit" ] && grep -q '^Sockets=.*kiln-door-proxy-public.socket' "$unit"; then
    sed -i 's/^Sockets=.*/Sockets=kiln-door-proxy.socket/' "$unit"
    systemctl daemon-reload
    changed=1
  fi
  state=$(systemctl show --property=LoadState --value kiln-door-proxy-public.socket 2>/dev/null || true)
  case $state in
    ""|not-found) ;;
    *)
      before=$(systemctl is-active kiln-door-proxy-public.socket 2>/dev/null || true)
      systemctl disable --now kiln-door-proxy-public.socket
      if [ "$before" = active ]; then
        changed=1
      fi
      ;;
  esac
  if [ "$changed" = 1 ] && systemctl is-active --quiet kiln-door-proxy.service; then
    systemctl restart kiln-door-proxy.service
  fi
}

upgrade() {
  local bundle=$1 address=${2:-} uplink=${3:-} network=${4:-0} apply=${5:-0}
  local pending name dest release target installed found reported count min_from
  local catalog same api_argv host_argv installed_address backup dir
  pending=$OPT/pending-upgrade.json
  if [ -e "$pending" ] || [ -L "$pending" ]; then
    die "pending update requires recovery: inspect $pending"
  fi
  for dir in "$ETC" "$STATE" "$OPT" "$OPT/bin" "$LIBEXEC"; do
    if [ ! -d "$dir" ] || [ -L "$dir" ]; then
      die "incomplete installation: $dir; recovery required"
    fi
    trusted_ancestors "$dir"
  done
  while IFS= read -r name; do
    need_file "$(dest_for "$name")" 1
    need_file "$bundle/bin/$name"
  done < <(binary_names)
  release=$bundle/release.json
  need_file "$release"
  target=$(json_string "$release" version || true)
  target=${target#v}
  is_version "$target" || die "unsupported update package"
  [ "$(json_string "$release" target || true)" = x86_64-unknown-linux-gnu ] || die "unsupported update package"
  installed=
  count=0
  while IFS= read -r name; do
    [ "$name" = kiln-network ] && continue
    reported=$(reported_name "$name")
    found=$(read_version "$(dest_for "$name")" "$reported") || die "unrecognized installed binary: $name"
    if [ -z "$installed" ]; then
      installed=$found
    elif [ "$found" != "$installed" ]; then
      die "mixed installed versions; no state migration is supported"
    fi
    found=$(read_version "$bundle/bin/$name" "$reported") || die "package binary version mismatch: $name"
    [ "$found" = "$target" ] || die "package binary version mismatch: $name"
    count=$((count + 1))
  done < <(binary_names)
  if [ "$count" -ne 4 ] || [ -z "$installed" ]; then
    die "mixed installed versions; no state migration is supported"
  fi
  if version_cmp "$installed" "$target"; then
    :
  else
    case $? in
      2) die "refusing downgrade from $installed to $target" ;;
      1) ;;
      *) die "unsupported update package" ;;
    esac
  fi
  if version_cmp "$installed" 0.3.0; then
    :
  else
    case $? in
      1) die "refusing upgrade from $installed; installs older than 0.3.0 are unsupported" ;;
      *) ;;
    esac
  fi
  min_from=$(json_string "$release" min_upgrade_from || true)
  min_from=${min_from#v}
  if [ -n "$min_from" ]; then
    is_version "$min_from" || die "unsupported update package"
    if version_cmp "$installed" "$min_from"; then
      :
    else
      case $? in
        1) die "refusing upgrade from $installed; package requires min_upgrade_from $min_from" ;;
        *) ;;
      esac
    fi
  fi
  local imaj imin tmaj tmin
  IFS=. read -r imaj imin _ <<<"$installed"
  IFS=. read -r tmaj tmin _ <<<"$target"
  if [ "$imaj" != "$tmaj" ] || [ "$imin" != "$tmin" ]; then
    [ -n "$min_from" ] || die "refusing minor or major upgrade from $installed to $target without min_upgrade_from"
  fi
  for name in kiln-host.service kiln-api.service; do
    [ "$(service_property "$name" ActiveState | tr -d '[:space:]')" = active ] || die "$name must be healthy and active before updating"
  done
  [ "$(service_property kiln-host.service KillMode | tr -d '[:space:]')" = process ] || die "kiln-host KillMode must be process to preserve guest VMMs"
  host_argv=$(service_argv kiln-host.service "$LIBEXEC/kiln-host")
  [ "$host_argv" = "$LIBEXEC/kiln-host --config $ETC/host.json" ] || die "unsupported host service configuration"
  need_file "$ETC/host.json"
  local runtime_dir journal_dir
  runtime_dir=$(json_string "$ETC/host.json" runtime_dir || true)
  journal_dir=$(json_string "$ETC/host.json" journal_dir || true)
  [ "$runtime_dir" = "$STATE/runtime" ] || die "updates require standard runtime and journal directories"
  [ "$journal_dir" = "$STATE/journal" ] || die "updates require standard runtime and journal directories"
  [ -d "$runtime_dir" ] || die "missing installed runtime_dir"
  [ -d "$journal_dir" ] || die "missing installed journal_dir"
  local iso socket
  iso=$(json_string "$ETC/host.json" isolation_config || true)
  socket=$(json_string "$ETC/host.json" socket || true)
  if [ "$iso" != "$ETC/isolation.json" ] || [ "$socket" != /run/kiln/host.sock ]; then
    die "unsupported host paths"
  fi
  api_argv=$(service_argv kiln-api.service "$LIBEXEC/kiln-api")
  read -r -a api_words <<<"$api_argv"
  if [ "${#api_words[@]}" -ne 11 ] || [ "${api_words[1]}" != --listen ] || [[ ${api_words[2]} != *:8443 ]]; then
    die "unsupported API service configuration"
  fi
  installed_address=${api_words[2]%:8443}
  installed_address=$(private_address "$installed_address")
  [ "$api_argv" = "$LIBEXEC/kiln-api --listen ${installed_address}:8443 --host-socket $socket --token-file $ETC/admin.token --tls-cert $ETC/tls.crt --tls-key $ETC/tls.key" ] \
    || die "unsupported API service configuration"
  if [ -n "$address" ] && [ "$(private_address "$address")" != "$installed_address" ]; then
    die "update preserves the existing address and TLS identity"
  fi
  need_file "$ETC/isolation.json"
  if [ "$network" = 1 ] || [ -n "$uplink" ]; then
    json_has_value "$ETC/isolation.json" network || die "update cannot enable networking in an existing store"
    if [ -n "$uplink" ]; then
      local net_argv
      net_argv=$(service_argv kiln-network.service "$OPT/bin/kiln-network")
      [ "$net_argv" = "$OPT/bin/kiln-network provision $uplink" ] || die "update cannot change the installed network uplink"
    fi
  fi
  have_tool sqlite3 || die "missing prerequisite: sqlite3; install host packages before retrying"
  catalog=$(healthcheck "$installed_address") || die "$HEALTH_ERROR"
  same=1
  while IFS= read -r name; do
    cmp -s "$(dest_for "$name")" "$bundle/bin/$name" || same=0
  done < <(binary_names)
  if [ "$same" = 1 ]; then
    if [ "$apply" = 1 ]; then
      close_public_door
    fi
    printf 'Kiln %s is already current; nothing changed.\n' "$target"
    connection_instructions "$installed_address"
    return 0
  fi
  printf 'Kiln %s -> %s: preserve configuration and VM state; briefly disconnect API/SSH sessions.\n' "$installed" "$target"
  if [ "$apply" != 1 ]; then
    printf 'Check only: rerun with sudo and --apply to update.\n'
    return 0
  fi
  trusted_ancestors "$OPT/upgrades"
  mkdir -p "$OPT/upgrades"
  chmod 700 "$OPT/upgrades"
  backup=$(mktemp -d "$OPT/upgrades/${installed}-XXXXXX")
  while IFS= read -r name; do
    atomic_copy "$(dest_for "$name")" "$backup/$name"
  done < <(binary_names)
  sync "$OPT/upgrades"
  (
    set -C
    printf '{"from": "%s", "to": "%s", "backup": "%s", "destinations": {"kiln": "%s", "kiln-runtime": "%s", "kiln-network": "%s", "kiln-host": "%s", "kiln-api": "%s"}}' \
      "$installed" "$target" "$backup" \
      "$OPT/bin/kiln" "$OPT/bin/kiln-runtime" "$OPT/bin/kiln-network" \
      "$LIBEXEC/kiln-host" "$LIBEXEC/kiln-api" >"$pending"
  )
  chmod 600 "$pending"
  sync "$pending"
  sync "$OPT"
  host_stopped=0
  if ! upgrade_swap "$catalog" "$installed_address"; then
    if ! upgrade_restore "$catalog" "$installed_address"; then
      die "update and rollback failed; manual recovery required using $pending and $backup"
    fi
    rm -f -- "$pending"
    sync "$OPT"
    die "update failed (command); previous binaries/services restored. Backup: $backup"
  fi
  rm -f -- "$pending"
  sync "$OPT"
  printf 'Binaries are already updated. Closing public tcp/8443. A failure here leaves those binaries in place.\n'
  close_public_door
  printf 'Updated to Kiln %s. Ready: https://%s:8443\nPrevious binaries: %s\n' "$target" "$installed_address" "$backup"
  connection_instructions "$installed_address"
}

fresh_plan() {
  local address=$1 uplink=${2:-}
  printf 'Checks passed. Proposed installation:\n'
  printf '  KVM host, endpoint https://%s:8443\n' "$address"
  printf '  /opt/kiln, /usr/local/libexec/kiln-host, /usr/local/libexec/kiln-api, /etc/kiln, /var/lib/kiln\n'
  printf '  kiln-api and eight locked VM accounts; UID 70000..70007 / GID 71000..71007\n'
  printf '  Enable cpu/memory/pids cgroup controllers and three systemd services\n'
  printf '  Generate private CA, one-year server certificate and administrator token\n'
  printf '  Build a 4 GiB / 1-vCPU warm Ubuntu template using the copy disk backend\n'
  printf '  No overlay-pool changes\n'
  printf 'Review any other runtime'"'"'s numeric ID reservations before proceeding.\n'
  if [ -n "$uplink" ]; then
    printf '  Enable IPv4 forwarding, filtered guest NAT via %s and a network service.\n' "$uplink"
  else
    printf '  No firewall, routing or guest-network changes.\n'
  fi
}

provision() {
  local address=${1:-} apply=${2:-0} uplink=${3:-} network=${4:-0}
  bundle=$(dirname "$(realpath "$0")")
  if [ -e "$ETC" ] || [ -L "$ETC" ] || [ -e "$OPT" ] || [ -L "$OPT" ] || [ -e "$STATE" ] || [ -L "$STATE" ]; then
    upgrade "$bundle" "$address" "$uplink" "$network" "$apply"
    return 0
  fi
  if [ "$network" = 1 ] && [ -z "$uplink" ]; then
    die "fresh manual installs need --network-uplink INTERFACE"
  fi
  if [ -z "$address" ]; then
    address=127.0.0.1
  fi
  address=$(private_address "$address")
  preflight "$bundle" "$address"
  if [ -n "$uplink" ]; then
    network_preflight "$uplink"
  fi
  fresh_plan "$address" "$uplink"
  if [ "$apply" != 1 ]; then
    printf 'Check only: no installation performed. Rerun with sudo and --apply to install.\n'
    return 0
  fi
  if [ -z "${KILN_INSTALL_FIXTURE:-}" ] && [ "$(id -u)" -ne 0 ]; then
    die "--apply requires sudo/root"
  fi
  install_fresh "$bundle" "$address" "$uplink"
}

main() {
  local address="" apply=0 uplink="" network=0 health_address=""
  while [ "$#" -gt 0 ]; do
    case $1 in
      --apply) apply=1; shift ;;
      --network) network=1; shift ;;
      --address)
        [ "$#" -ge 2 ] || die "usage: install.sh [--address IP] [--apply] [--network] [--network-uplink INTERFACE]"
        address=$2
        shift 2
        ;;
      --network-uplink)
        [ "$#" -ge 2 ] || die "usage: install.sh [--address IP] [--apply] [--network] [--network-uplink INTERFACE]"
        uplink=$2
        shift 2
        ;;
      --healthcheck)
        [ "$#" -ge 2 ] || die "usage: install.sh --healthcheck ADDRESS"
        health_address=$2
        shift 2
        ;;
      *) die "usage: install.sh [--address IP] [--apply] [--network] [--network-uplink INTERFACE]" ;;
    esac
  done
  if [ -n "$health_address" ]; then
    local body
    body=$(healthcheck "$health_address") || die "$HEALTH_ERROR"
    printf '%s\n' "$body"
    exit 0
  fi
  if [ "$apply" = 1 ] && [ -z "${KILN_INSTALL_FIXTURE:-}" ] && [ "$(id -u)" -ne 0 ]; then
    die "--apply requires sudo/root"
  fi
  acquire_lock
  provision "$address" "$apply" "$uplink" "$network"
}

main "$@"
