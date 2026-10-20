#!/bin/sh
# Unattended bootstrap for public Jabbslad/kiln releases.
# Keep all execution inside main: a truncated curl pipe must not start setup.
set +x
set -eu
umask 077
export LC_ALL=C

fail() { printf 'kiln: %s\n' "$*" >&2; exit 1; }

cleanup() {
    if [ -n "$staged" ]; then rm -f "$staged"; fi
    if [ -n "$backup_staged" ]; then rm -f "$backup_staged"; fi
    if [ -n "$work" ]; then rm -rf "$work"; fi
    if [ -n "$lock" ]; then rmdir "$lock" || :; fi
}

assets() {
    # Updated only after reviewing the release and its checksums.
    cat <<'ASSETS'
# BEGIN RELEASE ASSETS
client:x86_64-unknown-linux-gnu 8707652de26659f18f78e87142c21e6d42db385e31fc44d06cf7583c0a81bd35
client:x86_64-apple-darwin dd8c7e5267a22e0f4c4e4ed1344d8bb167a8b9bdca6700fd8af512a3b71fa308
client:aarch64-apple-darwin a565a8de22632d5f3363839297d6eee3a7420b35f17ea856b784a925e1c4b50a
server:x86_64-unknown-linux-gnu f8ebc05462c03ca42612872f8c9d39ed0ef1003a00d19572b64fca82cb483673
# END RELEASE ASSETS
ASSETS
}

platform() {
    case "$(uname -s):$(uname -m)" in
        Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
        Darwin:x86_64) target=x86_64-apple-darwin ;;
        Darwin:arm64) target=aarch64-apple-darwin ;;
        *) fail 'Supported clients: macOS ARM/Intel or Linux x86-64. Windows uses the release zip.' ;;
    esac
    if [ "$target" = x86_64-unknown-linux-gnu ]; then
        libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || :)
        printf '%s\n' "$libc" | awk '
            $1 == "glibc" && $2 ~ /^[0-9]+\.[0-9]+$/ {
                split($2, v, "."); if (v[1] > 2 || (v[1] == 2 && v[2] >= 39)) ok=1
            } END { exit !ok }' || fail 'Linux requires glibc 2.39 or newer.'
    fi
    if [ "$mode" = server ]; then
        [ "$target" = x86_64-unknown-linux-gnu ] || fail 'Server requires Ubuntu 24.04, Ubuntu 26.04, or Debian 13 x86-64.'
        [ -r /etc/os-release ] || fail 'Cannot identify server OS.'
        # shellcheck source=/dev/null
        . /etc/os-release
        case "${ID:-}:${VERSION_ID:-}" in
            ubuntu:24.04|ubuntu:26.04|debian:13) ;;
            *) fail 'Server requires Ubuntu 24.04, Ubuntu 26.04, or Debian 13 x86-64.' ;;
        esac
        [ -d /run/systemd/system ] || fail 'Server requires a booted systemd host.'
        # The provisioner checks actual KVM access under sudo, not this user's groups.
        [ -e /dev/kvm ] || fail 'Server requires /dev/kvm.'
        [ -r /sys/fs/cgroup/cgroup.controllers ] || fail 'Server requires cgroup v2.'
        for controller in cpu memory pids; do
            grep -qw "$controller" /sys/fs/cgroup/cgroup.controllers || fail "Missing cgroup controller: $controller"
        done
    fi
}

download() {
    printf 'Downloading kiln %s (%s)…\n' "$version" "$mode"
    package="kiln-v$version-$target.tar.gz"
    if [ "$mode" = server ]; then package="kiln-server-v$version-$target.tar.gz"; fi
    # No API lookup, credentials or curl user configuration. Redirects stay HTTPS.
    status=$(curl -q --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 30 --max-time 900 --output "$work/package.tar.gz" --write-out '%{http_code}' \
        "https://github.com/Jabbslad/kiln/releases/download/v$version/$package") || fail 'Package download failed.'
    [ "$status" = 200 ] || fail "Package download returned HTTP $status; check release availability."
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$work/package.tar.gz")
    else
        actual=$(shasum -a 256 "$work/package.tar.gz")
    fi
    actual=${actual%% *}
    [ "$actual" = "$digest" ] || fail 'Package checksum mismatch; nothing installed.'
}

extract() {
    # The packager emits only these regular files. Reject links, extra/duplicate
    # entries and path traversal before extraction, even after digest verification.
    tar -tzf "$work/package.tar.gz" > "$work/names" || fail 'Invalid package archive.'
    if [ "$mode" = client ]; then
        printf 'kiln\n' > "$work/expected"
    else
        grep -qx 'install.sh' "$work/names" || fail 'Unexpected archive contents.'
        cat > "$work/expected" <<FILES
bin/kiln-runtime
bin/kiln
bin/kiln-host
bin/kiln-api
install.sh
fetch-firecracker.sh
README.md
docs/releases.md
docs/remote-client.md
docs/runtime.md
docs/identity-service.md
docs/runbooks/2026-10-19-public-door.md
deploy/kiln-host.service
deploy/kiln-api.service
deploy/kiln-door.service
deploy/enroll-dark-forge.sh
deploy/host.example.json
image/image.json
image/inputs.json
image/vmlinux
image/rootfs.ext4
release.json
FILES
        # Guest-access releases add one fixed-purpose network helper. Older pinned
        # packages remain installable, but cannot opt in to networking.
        if grep -qx 'bin/kiln-network' "$work/names"; then
            printf 'bin/kiln-network\n' >> "$work/expected"
        fi
        # Door proxy units and their runbook ship together. An archive that
        # has any of these files must include the private unit and the
        # runbook. The public unit is optional. An older archive may still
        # carry it.
        if grep -qx 'deploy/kiln-door-proxy.service' "$work/names" \
            || grep -qx 'deploy/kiln-door-proxy.socket' "$work/names" \
            || grep -qx 'deploy/kiln-door-proxy-public.socket' "$work/names" \
            || grep -qx 'docs/runbooks/2026-10-19-door-public-cert.md' "$work/names"
        then
            printf '%s\n' \
                deploy/kiln-door-proxy.service \
                deploy/kiln-door-proxy.socket \
                docs/runbooks/2026-10-19-door-public-cert.md >> "$work/expected"
        fi
        if grep -qx 'deploy/kiln-door-proxy-public.socket' "$work/names"; then
            printf '%s\n' deploy/kiln-door-proxy-public.socket >> "$work/expected"
        fi
        # The paseo Caddy override and the door site are their own pair.
        # v0.4.9 has the four door files and neither of these. An archive
        # that has one of them must have both, with or without the door files.
        if grep -qx 'deploy/paseo-caddy.override.yaml' "$work/names" \
            || grep -qx 'deploy/door.Caddyfile' "$work/names"
        then
            printf '%s\n' \
                deploy/paseo-caddy.override.yaml \
                deploy/door.Caddyfile >> "$work/expected"
        fi
        # The app listener units ship together. An older archive has none.
        # An archive that has any of them must have all three.
        if grep -qx 'deploy/kiln-app.service' "$work/names" \
            || grep -qx 'deploy/kiln-app-proxy.service' "$work/names" \
            || grep -qx 'deploy/kiln-app-proxy.socket' "$work/names"
        then
            printf '%s\n' \
                deploy/kiln-app.service \
                deploy/kiln-app-proxy.service \
                deploy/kiln-app-proxy.socket >> "$work/expected"
        fi
        # The app site and its runbook ship together. An older archive has
        # neither. An archive that has one of them must have both.
        if grep -qx 'deploy/app.Caddyfile' "$work/names" \
            || grep -qx 'docs/runbooks/2026-10-20-app-dark-forge-caddy.md' "$work/names"
        then
            printf '%s\n' \
                deploy/app.Caddyfile \
                docs/runbooks/2026-10-20-app-dark-forge-caddy.md >> "$work/expected"
        fi
    fi
    sort "$work/expected" > "$work/expected.sorted"
    sort "$work/names" > "$work/names.sorted"
    cmp -s "$work/expected.sorted" "$work/names.sorted" || fail 'Unexpected archive contents.'
    tar -tvzf "$work/package.tar.gz" > "$work/types" || fail 'Invalid package archive.'
    awk 'substr($0,1,1) != "-" { bad=1 } END { exit bad }' "$work/types" || fail 'Archive contains non-regular files.'
    mkdir "$work/package"
    tar -xzf "$work/package.tar.gz" -C "$work/package" || fail 'Cannot extract package archive.'
}

as_root() {
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo -n "$@"; fi
}

resource_preflight() {
    awk '/^MemAvailable:/ {kb=$2} END {exit !(kb+0 >= 6*1024*1024)}' /proc/meminfo \
        || fail 'at least 6 GiB available host RAM is required for the initial 4 GiB template'
    avail=$(df -B1 /var/lib | awk 'NR==2 {print $4}')
    [ "${avail:-0}" -ge $((24 * 1024 * 1024 * 1024)) ] \
        || fail 'at least 24 GiB free on /var/lib is required for initial copy-backend storage'
    as_root sh -c '[ -r /dev/kvm ] && [ -w /dev/kvm ]' < /dev/null \
        || fail 'Server requires read-write access to /dev/kvm.'
}

check_upgrade_paths() {
    if [ ! -f "$destination" ] || [ -L "$destination" ]; then
        fail 'Upgrade requires an existing regular file at ~/.local/bin/kiln; symbolic links are refused.'
    fi
    if [ -e "$destination.previous" ] || [ -L "$destination.previous" ]; then
        if [ ! -f "$destination.previous" ] || [ -L "$destination.previous" ]; then
            fail 'Backup destination must be a regular file, not a symbolic link or directory.'
        fi
    fi
}

install_client() {
    found=$("$work/package/kiln" --version) || fail 'Downloaded client cannot run on this machine.'
    [ "$found" = "kiln $version" ] || fail 'Downloaded client version does not match release.'
    if [ "$upgrade" = true ] && cmp -s "$destination" "$work/package/kiln"; then
        printf 'kiln %s is already current; previous backup preserved.\n' "$version"
        printf '%s\n' 'Next: run kiln login to sign in with GitHub or Google. Your server must be enrolled first.'
        return
    fi
    mkdir -p "$HOME/.local/bin"
    staged=$(mktemp "$HOME/.local/bin/.kiln.XXXXXXXX")
    cp "$work/package/kiln" "$staged"
    chmod 755 "$staged"
    if [ "$upgrade" = true ]; then
        check_upgrade_paths
        backup_staged=$(mktemp "$HOME/.local/bin/.kiln-backup.XXXXXXXX")
        cp -p "$destination" "$backup_staged" || fail 'Cannot back up existing client; nothing replaced.'
        mv -f "$backup_staged" "$destination.previous" || fail 'Cannot publish client backup; nothing replaced.'
        backup_staged=
        # Same-filesystem rename: readers see either the old or verified binary.
        mv -f "$staged" "$destination" || fail 'Cannot replace client; existing binary preserved.'
        printf 'Previous client retained at %s.previous\n' "$destination"
    else
        # Fresh installs never overwrite a file created by another process.
        ln "$staged" "$destination" || fail 'Client destination exists; refusing to overwrite.'
        rm -f "$staged"
    fi
    staged=
    printf 'Installed %s at %s/.local/bin/kiln\n' "$found" "$HOME"
    # shellcheck disable=SC2016
    case ":$PATH:" in
        *":$HOME/.local/bin:"*) ;;
        *) printf '%s\n' 'Add to your shell PATH: export PATH="$HOME/.local/bin:$PATH"' ;;
    esac
    printf '%s\n' 'Next: run kiln login to sign in with GitHub or Google. Your server must be enrolled first.'
    printf '%s\n' 'Existing administrator profiles are preserved; use kiln --profile personal login for a separate browser-login profile.'
}

configure_server() {
    if [ -z "$address" ] || { [ "$network" = true ] && [ -z "$uplink" ]; }; then
        command -v ip >/dev/null 2>&1 || fail 'Address/uplink detection requires iproute2; install it or specify --address and --network-uplink.'
        # Kernel route lookup only: this sends no traffic and honors route metrics.
        route=$(ip -4 route get 1.1.1.1) || fail 'Cannot detect route; specify --address and, if networking is enabled, --network-uplink.'
        if [ -z "$address" ]; then
            address=$(printf '%s\n' "$route" | awk '{for(i=1;i<NF;i++) if($i=="src") {value=$(i+1); n++}} END {if(n==1) print value}')
            [ -n "$address" ] || fail 'Cannot detect one IPv4 address; specify --address.'
        fi
        if [ "$network" = true ] && [ -z "$uplink" ]; then
            uplink=$(printf '%s\n' "$route" | awk '{for(i=1;i<NF;i++) if($i=="dev") {value=$(i+1); n++}} END {if(n==1) print value}')
            [ -n "$uplink" ] || fail 'Cannot detect one uplink; specify --network-uplink.'
        fi
    fi
    # Validate before apt mutations. The installer rechecks the address and the bind.
    printf '%s\n' "$address" | awk -F . '
        NF == 4 {
            for(i=1;i<=4;i++) if($i !~ /^(0|[1-9][0-9]*)$/ || $i>255) exit 1
            if($1==10 || ($1==172 && $2>=16 && $2<=31) || ($1==192 && $2==168) ||
               ($1==100 && $2>=64 && $2<=127) || ($1==127 && $2==0 && $3==0 && $4==1)) ok=1
        } END { exit !(ok && NR==1) }' || fail 'Use --address with a private/VPN IPv4 address assigned to this server.'
    if [ -n "$uplink" ]; then
        case "$uplink" in *[!A-Za-z0-9_.:-]*) fail 'Invalid network uplink interface.' ;; esac
        printf '%s\n' "$uplink" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,14}$' || fail 'Invalid network uplink interface.'
    fi
    printf 'Server endpoint: https://%s:8443\n' "$address"
}

install_sqlite3_if_missing() {
    if ! command -v sqlite3 >/dev/null 2>&1; then
        as_root env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update < /dev/null
        as_root env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3 < /dev/null
    fi
}

run_installer() {
    [ -f "$work/package/install.sh" ] || fail 'Package has no installer.'
    as_root bash "$work/package/install.sh" "$@" < /dev/null
}

retire_public_door() {
    as_root sh -s <<'EOF'
unit=/etc/systemd/system/kiln-door-proxy.service
changed=0
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
    if [ "$before" = active ]; then changed=1; fi
    ;;
esac
if [ "$changed" = 1 ] && systemctl is-active --quiet kiln-door-proxy.service; then
  systemctl restart kiln-door-proxy.service
fi
EOF
}

install_server() {
    if [ "$existing_server" = true ]; then
        install_sqlite3_if_missing
        retire_public_door
        set -- --apply
        if [ -n "$address" ]; then set -- "$@" --address "$address"; fi
        if [ -n "$uplink" ]; then set -- "$@" --network-uplink "$uplink"; fi
        if [ "$network" = true ]; then set -- "$@" --network; fi
        run_installer "$@"
        return
    fi
    if [ -n "$uplink" ]; then
        [ -f "$work/package/bin/kiln-network" ] || fail 'This pinned release has no guest networking; a guest-access release is required.'
        printf 'Networking requested: enable host IPv4 forwarding and filtered NAT via %s.\n' "$uplink"
    fi
    resource_preflight
    set -- openssl curl ca-certificates tar passwd
    notice="$*"
    if ! command -v sqlite3 >/dev/null 2>&1; then
        notice="sqlite3 $notice"
    fi
    printf '%s\n' \
        'Fresh Kiln installation. Fresh-host/reboot validation is outstanding.' \
        "Setup will use sudo to install Ubuntu packages: $notice." \
        'The server command authorizes setup; resource/conflict checks still run before creating kiln accounts and services.' \
        'A failed setup retains runtime state for diagnosis; do not delete it and blindly retry.'
    as_root env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get update < /dev/null
    if [ -n "$uplink" ]; then
        set -- "$@" iproute2 nftables util-linux
    fi
    as_root env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get install --no-install-recommends -y \
        -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold "$@" < /dev/null
    install_sqlite3_if_missing
    set -- --address "$address" --apply
    if [ -n "$uplink" ]; then set -- "$@" --network-uplink "$uplink"; fi
    run_installer "$@"
}

main() {
    version=0.4.10
    work='' staged='' backup_staged='' lock='' upgrade=false
    address='' uplink=${KILN_NETWORK_UPLINK:-} network=false existing_server=false
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    usage='Usage: sh install.sh [client|server [--address IP] [--network|--network-uplink INTERFACE]] (installs or updates automatically)'
    mode=${1:-client}
    case "$mode" in
        client|server) ;;
        --help|-h) printf '%s\n' "$usage" 'No prompts or GitHub token. Server setup requires root or passwordless sudo and authorizes installation.'; return ;;
        *) fail "$usage" ;;
    esac
    if [ "$#" -gt 0 ]; then shift; fi
    while [ "$#" -gt 0 ]; do
        case "$mode:$1" in
            client:--upgrade) shift ;; # Backward-compatible alias; never required.
            server:--address|server:--network-uplink)
                [ "$#" -ge 2 ] || fail "$usage"
                [ -n "$2" ] || fail "$usage"
                if [ "$1" = --address ]; then address=$2; else uplink=$2; fi
                shift 2 ;;
            server:--network) network=true; shift ;;
            *) fail "$usage" ;;
        esac
    done
    platform
    for tool in curl tar awk sort cmp mktemp grep; do
        command -v "$tool" >/dev/null 2>&1 || fail "Missing standard system tool: $tool"
    done
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || fail 'Missing system SHA-256 tool.'
    if [ "$mode" = client ]; then
        [ -n "${HOME:-}" ] || fail 'HOME must be set.'
        destination="$HOME/.local/bin/kiln"
        mkdir -p "$HOME/.local/bin"
        mkdir "$HOME/.local/bin/.kiln-install.lock" 2>/dev/null || fail 'Cannot lock client destination; another installer may be running. Inspect ~/.local/bin/.kiln-install.lock before removing a stale lock.'
        lock="$HOME/.local/bin/.kiln-install.lock"
        if [ -e "$destination" ] || [ -L "$destination" ]; then
            upgrade=true
            check_upgrade_paths
            current=$("$destination" --version) || fail 'Existing file is not a runnable Kiln client; refusing to overwrite.'
            printf '%s\n' "$current" | awk '/^kiln [0-9]+\.[0-9]+\.[0-9]+$/ {ok++} END {exit !(NR==1 && ok==1)}' || fail 'Unrecognized installed client; refusing to overwrite.'
            awk -v current="$current" -v target="$version" 'BEGIN {
                sub(/^kiln /,"",current); split(current,a,"."); split(target,b,".");
                for(i=1;i<=3;i++) {if(a[i]+0>b[i]+0) exit 1; if(a[i]+0<b[i]+0) exit 0}
            }' || fail 'Installed client is newer than this installer; refusing to downgrade.'
        fi
    else
        for path in /etc/kiln /opt/kiln /var/lib/kiln; do
            if [ -e "$path" ] || [ -L "$path" ]; then existing_server=true; fi
        done
        if [ "$existing_server" = false ]; then configure_server; fi
        if [ "$(id -u)" != 0 ]; then
            command -v sudo >/dev/null 2>&1 || fail 'Server setup needs passwordless sudo or a root shell.'
            sudo -n true < /dev/null || fail 'Server setup needs passwordless sudo or a root shell; no password prompts are used.'
        fi
    fi
    digest=$(assets | awk -v key="$mode:$target" '$1==key {print $2}')
    case "$digest" in ''|*[!a-f0-9]*) fail 'Invalid pinned release digest.' ;; esac
    [ "${#digest}" = 64 ] || fail 'Invalid pinned release digest.'
    work=$(mktemp -d "${TMPDIR:-/tmp}/kiln-install.XXXXXXXX")
    download
    extract
    if [ "$mode" = client ]; then install_client; else install_server; fi
}

main "$@"
