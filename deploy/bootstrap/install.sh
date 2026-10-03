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
client:x86_64-unknown-linux-gnu 0f333b72dea810d976cc15b14dd6c43e8995de41fc20b1b54e43e9897a048a68
client:x86_64-apple-darwin 70d60d96d55e92371e1950b2f44ee7cc15705f5f9119a9702bd818d4b51e6a1e
client:aarch64-apple-darwin 88e0bddb889155db223ffcbcc3fe19c43ee4b88800adc0cd8c3fd55ea005a719
server:x86_64-unknown-linux-gnu ff00fea59b99f5523ef69b75dd890b916da58cc88b57dfa8e0f13051374520b8
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
        [ "$target" = x86_64-unknown-linux-gnu ] || fail 'Server requires Ubuntu 24.04 or 26.04 x86-64.'
        [ -r /etc/os-release ] || fail 'Cannot identify server OS.'
        # shellcheck source=/dev/null
        . /etc/os-release
        case "${ID:-}:${VERSION_ID:-}" in
            ubuntu:24.04|ubuntu:26.04) ;;
            *) fail 'Server requires Ubuntu 24.04 or 26.04 x86-64.' ;;
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
    if [ "$mode" = client ]; then
        printf 'kiln\n' > "$work/expected"
    else
        cat > "$work/expected" <<'FILES'
bin/kiln-runtime
bin/kiln
bin/kiln-host
bin/kiln-api
install.py
fetch-firecracker.sh
README.md
docs/releases.md
docs/remote-client.md
docs/runtime.md
deploy/kiln-host.service
deploy/kiln-api.service
deploy/host.example.json
image/image.json
image/inputs.json
image/vmlinux
image/rootfs.ext4
release.json
FILES
    fi
    tar -tzf "$work/package.tar.gz" > "$work/names" || fail 'Invalid package archive.'
    # Guest-access releases add one fixed-purpose network helper. Older pinned
    # packages remain installable, but cannot opt in to networking.
    if [ "$mode" = server ] && grep -qx 'bin/kiln-network' "$work/names"; then
        printf 'bin/kiln-network\n' >> "$work/expected"
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
    printf '%s\n' 'Next: connect using the server enrollment bundle and its CONNECT.txt instructions.'
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
    # Validate before apt mutations; the Python provisioner rechecks this and binding.
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

install_server() {
    if [ -n "$uplink" ]; then
        [ -f "$work/package/bin/kiln-network" ] || fail 'This pinned release has no guest networking; a guest-access release is required.'
        printf 'Networking requested: enable host IPv4 forwarding and filtered NAT via %s.\n' "$uplink"
    fi
    printf '%s\n' \
        'This is a fresh-install pilot, not an upgrader. Fresh-host/reboot validation is outstanding.' \
        'Setup will use sudo to install Ubuntu packages: python3 openssl curl ca-certificates tar passwd.' \
        'The server command authorizes setup; resource/conflict checks still run before creating kiln accounts and services.' \
        'A failed setup retains runtime state for diagnosis; do not delete it and blindly retry.'
    as_root env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get update < /dev/null
    set -- python3 openssl curl ca-certificates tar passwd
    if [ -n "$uplink" ]; then
        set -- "$@" iproute2 nftables util-linux
    fi
    as_root env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a apt-get install --no-install-recommends -y \
        -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold "$@" < /dev/null
    set -- --address "$address" --apply
    if [ -n "$uplink" ]; then set -- "$@" --network-uplink "$uplink"; fi
    as_root python3 "$work/package/install.py" "$@" < /dev/null
}

main() {
    version=0.3.2
    work='' staged='' backup_staged='' lock='' upgrade=false
    address='' uplink=${KILN_NETWORK_UPLINK:-} network=false
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    usage='Usage: sh install.sh [client [--upgrade]|server [--address IP] [--network|--network-uplink INTERFACE]]'
    mode=${1:-client}
    case "$mode" in
        client|server) ;;
        --help|-h) printf '%s\n' "$usage" 'No prompts or GitHub token. Server setup requires root or passwordless sudo and authorizes installation.'; return ;;
        *) fail "$usage" ;;
    esac
    if [ "$#" -gt 0 ]; then shift; fi
    while [ "$#" -gt 0 ]; do
        case "$mode:$1" in
            client:--upgrade) upgrade=true; shift ;;
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
        if [ "$upgrade" = true ]; then
            check_upgrade_paths
        elif [ -e "$destination" ] || [ -L "$destination" ]; then
            fail 'Existing kiln preserved; rerun with: sh -s -- client --upgrade'
        fi
        mkdir -p "$HOME/.local/bin"
        mkdir "$HOME/.local/bin/.kiln-install.lock" 2>/dev/null || fail 'Cannot lock client destination; another installer may be running. Inspect ~/.local/bin/.kiln-install.lock before removing a stale lock.'
        lock="$HOME/.local/bin/.kiln-install.lock"
    else
        configure_server
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
