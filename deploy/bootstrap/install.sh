#!/bin/sh
# Public bootstrap only. Packages and source remain in private Jabbslad/boxd.
# Keep all execution inside main: a truncated curl pipe must not start setup.
set +x
set -eu
umask 077
export LC_ALL=C

fail() { printf 'boxd: %s\n' "$*" >&2; exit 1; }

cleanup() {
    if [ -n "$tty_state" ]; then stty "$tty_state" < /dev/tty || :; fi
    if [ -n "$staged" ]; then rm -f "$staged"; fi
    if [ -n "$work" ]; then rm -rf "$work"; fi
}

assets() {
    # Updated only after reviewing the private release and its checksums.
    cat <<'ASSETS'
# BEGIN RELEASE ASSETS
client:x86_64-unknown-linux-gnu 606436787 74181d6bfd987ccf6a26b9955d2bad53ed78ea09850ba14d185fd2e0458cb71d
client:x86_64-apple-darwin 606436766 e080c664afec9e975cbb7ee77e1fc4124083c0bca09e9bc0f6858368ff168403
client:aarch64-apple-darwin 606436770 4ca7db7e13a9fdb91c956475863f17dead298563fe23aae341b6e677b146445b
server:x86_64-unknown-linux-gnu 606436793 ec9d4622a0b942770842dd94da20d45abe08ec8db9b66f7c7f9d0fdff34580e5
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
    printf '%s\n' 'Private package: use a fine-grained GitHub token for Jabbslad/boxd with Contents: read.'
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
    printf 'Downloading boxd %s (%s)…\n' "$version" "$mode"
    # Never follow a redirect with the credential-bearing curl configuration.
    status=$(curl -q --silent --show-error --proto '=https' --connect-timeout 30 --max-time 900 \
        --config "$work/auth.conf" --header 'Accept: application/octet-stream' \
        --header 'X-GitHub-Api-Version: 2022-11-28' \
        --dump-header "$work/headers" --output "$work/package.tar.gz" --write-out '%{http_code}' \
        "https://api.github.com/repos/Jabbslad/boxd/releases/assets/$asset") || fail 'Package download failed.'
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
        *) fail "GitHub returned HTTP $status. Check token access to Jabbslad/boxd and release availability." ;;
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
        printf 'boxctl\n' > "$work/expected"
    else
        cat > "$work/expected" <<'FILES'
bin/box
bin/boxctl
bin/boxd-host
bin/boxd-api
install.py
fetch-firecracker.sh
README.md
docs/releases.md
docs/remote-client.md
docs/runtime.md
deploy/boxd-host.service
deploy/boxd-api.service
deploy/host.example.json
image/image.json
image/inputs.json
image/vmlinux
image/rootfs.ext4
release.json
FILES
    fi
    tar -tzf "$work/package.tar.gz" > "$work/names" || fail 'Invalid package archive.'
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

install_client() {
    found=$("$work/package/boxctl" --version) || fail 'Downloaded client cannot run on this machine.'
    [ "$found" = "box-client $version" ] || fail 'Downloaded client version does not match release.'
    mkdir -p "$HOME/.local/bin"
    staged=$(mktemp "$HOME/.local/bin/.boxctl.XXXXXXXX")
    cp "$work/package/boxctl" "$staged"
    chmod 755 "$staged"
    # Atomic, no-overwrite publication on the destination filesystem.
    ln "$staged" "$HOME/.local/bin/boxctl" || fail 'Client destination exists; refusing to overwrite.'
    rm -f "$staged"
    staged=
    printf 'Installed %s at %s/.local/bin/boxctl\n' "$found" "$HOME"
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
    printf '%s\n' \
        'This is a fresh-install pilot, not an upgrader. Fresh-host/reboot validation is outstanding.' \
        'Setup will use sudo to install Ubuntu packages: python3 openssl curl ca-certificates tar passwd.' \
        'It will then check resources/conflicts and ask INSTALL before creating boxd accounts and services.' \
        'A failed setup retains runtime state for diagnosis; do not delete it and blindly retry.'
    printf 'Type SETUP to install prerequisites and continue: ' > /dev/tty
    IFS= read -r confirmation < /dev/tty || fail 'Setup cancelled.'
    [ "$confirmation" = SETUP ] || fail 'Cancelled; no system changes made.'
    as_root apt-get update < /dev/tty
    as_root apt-get install --no-install-recommends -y python3 openssl curl ca-certificates tar passwd < /dev/tty
    as_root python3 "$work/package/install.py" --address "$address" --apply < /dev/tty
}

main() {
    version=0.1.1
    work='' staged='' tty_state=''
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    [ "$#" -le 1 ] || fail 'Usage: sh install.sh [client|server]'
    mode=${1:-client}
    case "$mode" in
        client|server) ;;
        --help|-h) printf '%s\n' 'Usage: sh install.sh [client|server]. Requires a terminal and private GitHub package access.'; return ;;
        *) fail 'Usage: sh install.sh [client|server]' ;;
    esac
    platform
    for tool in curl tar awk sed tr sort cmp mktemp stty; do
        command -v "$tool" >/dev/null 2>&1 || fail "Missing standard system tool: $tool"
    done
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || fail 'Missing system SHA-256 tool.'
    if [ "$mode" = client ]; then
        [ -n "${HOME:-}" ] || fail 'HOME must be set.'
        if [ -e "$HOME/.local/bin/boxctl" ] || [ -L "$HOME/.local/bin/boxctl" ]; then
            fail 'Existing boxctl preserved; automatic upgrades are not supported.'
        fi
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
    work=$(mktemp -d "${TMPDIR:-/tmp}/boxd-install.XXXXXXXX")
    download
    extract
    if [ "$mode" = client ]; then install_client; else install_server; fi
}

main "$@"
