#!/bin/sh
# Public bootstrap only. Packages and source remain in private Jabbslad/kiln.
# Keep all execution inside main: a truncated curl pipe must not start setup.
set +x
set -eu
umask 077
export LC_ALL=C

fail() { printf 'kiln: %s\n' "$*" >&2; exit 1; }

cleanup() {
    if [ -n "$tty_state" ]; then stty "$tty_state" < /dev/tty || :; fi
    if [ -n "$staged" ]; then rm -f "$staged"; fi
    if [ -n "$backup_staged" ]; then rm -f "$backup_staged"; fi
    if [ -n "$work" ]; then rm -rf "$work"; fi
    if [ -n "$lock" ]; then rmdir "$lock" || :; fi
}

assets() {
    # Updated only after reviewing the private release and its checksums.
    cat <<'ASSETS'
# BEGIN RELEASE ASSETS
client:x86_64-unknown-linux-gnu 606745950 74d534eda055742951a2bf2c7b3ebc3a51d88905abd12e845e02d3a40ea0d629
client:x86_64-apple-darwin 606745907 230dd28f9237c21504c3a09f21eaf60904baf7a6e69562dc013d2e37d0102cb1
client:aarch64-apple-darwin 606745911 79c5e2d79c9c83cd749c812b2c9338cd8adfc311bcf0da35045d560a43464502
server:x86_64-unknown-linux-gnu 606745906 749b4bd568cd970a9df5d652329bf8e05d2774f03acbb56bb8d908dc6847063b
# END RELEASE ASSETS
ASSETS
}

platform() {
    case "$(uname -s):$(uname -m)" in
        Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
        Darwin:x86_64) target=x86_64-apple-darwin ;;
        Darwin:arm64) target=aarch64-apple-darwin ;;
        *) fail 'Supported clients: macOS ARM/Intel or Linux x86-64. Windows uses the private release zip.' ;;
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
    printf '%s\n' 'Private package: use a fine-grained GitHub token for Jabbslad/kiln with Contents: read.'
    tty_state=$(stty -g < /dev/tty)
    stty -echo < /dev/tty
    printf 'GitHub token: ' > /dev/tty
    IFS= read -r token < /dev/tty || fail 'Token input cancelled.'
    stty "$tty_state" < /dev/tty
    tty_state=
    printf '\n' > /dev/tty
    case "$token" in ''|*[!A-Za-z0-9_]*) fail 'Invalid GitHub token format.' ;; esac
    printf 'header = "Authorization: Bearer %s"\n' "$token" > "$work/auth.conf"
    unset token
    printf 'Downloading kiln %s (%s)…\n' "$version" "$mode"
    # Never follow a redirect with the credential-bearing curl configuration.
    status=$(curl -q --silent --show-error --proto '=https' --connect-timeout 30 --max-time 900 \
        --config "$work/auth.conf" --header 'Accept: application/octet-stream' \
        --header 'X-GitHub-Api-Version: 2022-11-28' \
        --dump-header "$work/headers" --output "$work/package.tar.gz" --write-out '%{http_code}' \
        "https://api.github.com/repos/Jabbslad/kiln/releases/assets/$asset") || fail 'Package download failed.'
    rm -f "$work/auth.conf"
    case "$status" in
        200) ;;
        302)
            location=$(sed -n 's/^[Ll][Oo][Cc][Aa][Tt][Ii][Oo][Nn]: *//p' "$work/headers" | tr -d '\r')
            case "$location" in
                https://release-assets.githubusercontent.com/*) ;;
                *) fail 'Unexpected package redirect; refusing download.' ;;
            esac
            # Signed asset URLs are temporary credentials too: keep them out of argv.
            case "$location" in *\"*|*\\*|*'
'*) fail 'Invalid package redirect.' ;; esac
            printf 'url = "%s"\n' "$location" > "$work/asset.conf"
            status=$(curl -q --silent --show-error --proto '=https' --connect-timeout 30 --max-time 900 \
                --config "$work/asset.conf" --output "$work/package.tar.gz" --write-out '%{http_code}') || fail 'Asset download failed.'
            rm -f "$work/asset.conf"
            [ "$status" = 200 ] || fail "Asset download returned HTTP $status."
            ;;
        *) fail "GitHub returned HTTP $status. Check token access to Jabbslad/kiln and release availability." ;;
    esac
    rm -f "$work/headers"
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
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi
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

install_server() {
    printf 'Server private IPv4 address: ' > /dev/tty
    IFS= read -r address < /dev/tty || fail 'Address input cancelled.'
    # Validate before apt mutations; the Python provisioner rechecks this and binding.
    printf '%s\n' "$address" | awk -F . '
        NF == 4 {
            for(i=1;i<=4;i++) if($i !~ /^(0|[1-9][0-9]*)$/ || $i>255) exit 1
            if($1==10 || ($1==172 && $2>=16 && $2<=31) || ($1==192 && $2==168) ||
               ($1==100 && $2>=64 && $2<=127) || ($1==127 && $2==0 && $3==0 && $4==1)) ok=1
        } END { exit !ok }' || fail 'Use a private/VPN IPv4 address assigned to this server.'
    uplink=${KILN_NETWORK_UPLINK:-}
    if [ -n "$uplink" ]; then
        printf '%s\n' "$uplink" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9_.:-]{0,14}$' || fail 'Invalid network uplink interface.'
        [ -f "$work/package/bin/kiln-network" ] || fail 'This pinned release has no guest networking; a guest-access release is required.'
        printf 'Networking requested: enable host IPv4 forwarding and filtered NAT via %s.\n' "$uplink"
    fi
    printf '%s\n' \
        'This is a fresh-install pilot, not an upgrader. Fresh-host/reboot validation is outstanding.' \
        'Setup will use sudo to install Ubuntu packages: python3 openssl curl ca-certificates tar passwd.' \
        'It will then check resources/conflicts and ask INSTALL before creating kiln accounts and services.' \
        'A failed setup retains runtime state for diagnosis; do not delete it and blindly retry.'
    printf 'Type SETUP to install prerequisites and continue: ' > /dev/tty
    IFS= read -r confirmation < /dev/tty || fail 'Setup cancelled.'
    [ "$confirmation" = SETUP ] || fail 'Cancelled; no system changes made.'
    as_root apt-get update < /dev/tty
    if [ -n "$uplink" ]; then
        as_root apt-get install --no-install-recommends -y python3 openssl curl ca-certificates tar passwd iproute2 nftables util-linux < /dev/tty
        as_root python3 "$work/package/install.py" --address "$address" --apply --network-uplink "$uplink" < /dev/tty
    else
        as_root apt-get install --no-install-recommends -y python3 openssl curl ca-certificates tar passwd < /dev/tty
        as_root python3 "$work/package/install.py" --address "$address" --apply < /dev/tty
    fi
}

main() {
    version=0.3.0
    work='' staged='' backup_staged='' tty_state='' lock='' upgrade=false
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    usage='Usage: sh install.sh [client [--upgrade]|server]'
    [ "$#" -le 2 ] || fail "$usage"
    mode=${1:-client}
    case "$mode" in
        client|server) ;;
        --help|-h) printf '%s\n' "$usage. Requires a terminal and private GitHub package access."; return ;;
        *) fail "$usage" ;;
    esac
    if [ "$#" = 2 ]; then
        if [ "$mode" != client ] || [ "$2" != --upgrade ]; then fail "$usage"; fi
        upgrade=true
    fi
    platform
    for tool in curl tar awk sed tr sort cmp mktemp stty grep; do
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
    elif [ "$(id -u)" != 0 ]; then
        command -v sudo >/dev/null 2>&1 || fail 'Server setup needs sudo or a root shell.'
    fi
    row=$(assets | awk -v key="$mode:$target" '$1==key {print $2, $3}')
    asset=${row%% *}
    digest=${row#* }
    case "$asset" in ''|*[!0-9]*) fail 'No pinned release asset for this platform.' ;; esac
    case "$digest" in ''|*[!a-f0-9]*) fail 'Invalid pinned release digest.' ;; esac
    [ "${#digest}" = 64 ] || fail 'Invalid pinned release digest.'
    # Verify a controlling terminal before making private temporary files.
    ( : < /dev/tty ) 2>/dev/null || fail 'Run interactively in a terminal to enter your GitHub token.'
    work=$(mktemp -d "${TMPDIR:-/tmp}/kiln-install.XXXXXXXX")
    download
    extract
    if [ "$mode" = client ]; then install_client; else install_server; fi
}

main "$@"
