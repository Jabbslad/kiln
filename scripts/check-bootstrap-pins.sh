#!/bin/sh
# Compare deploy/bootstrap/install.sh with the published release archives.
set -eu

pin_fail() {
    printf 'kiln: %s\n' "$*" >&2
    return 1
}

pin_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    else
        shasum -a 256 "$1" | awk '{ print $1 }'
    fi
}

pin_parse_docs() {
    docs=$1
    matched=$(sed -n "s/.*gh attestation verify ARCHIVE --repo \\([^ ]*\\) --cert-identity-regex '\\([^']*\\)'.*/\\1 \\2/p" "$docs")
    count=$(printf '%s\n' "$matched" | grep -c .)
    if [ "$count" -ne 1 ]; then
        pin_fail 'docs/releases.md must document one gh attestation verify command'
        return 1
    fi
    PIN_REPO=${matched%% *}
    PIN_IDENTITY=${matched#* }
    case $PIN_REPO in
        */*/*) pin_fail 'documented repo'; return 1 ;;
        [A-Za-z0-9_.-]*/[A-Za-z0-9_.-]*) ;;
        *) pin_fail 'documented repo'; return 1 ;;
    esac
    if printf '%s\n' x | grep -E "$PIN_IDENTITY" >/dev/null 2>&1; then
        :
    else
        status=$?
        if [ "$status" -eq 2 ]; then
            pin_fail 'documented cert-identity regex'
            return 1
        fi
    fi
}

pin_parse_install() {
    install=$1
    version=$(sed -n 's/^    version=\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)$/\1/p' "$install")
    count=$(printf '%s\n' "$version" | grep -c . || true)
    if [ "$count" -ne 1 ]; then
        pin_fail 'install.sh must pin one release version'
        return 1
    fi
    PIN_VERSION=$version
    rows=$(sed -n 's/^\(client\|server\):\([A-Za-z0-9._-]*\) \([0-9a-f]\{64\}\)$/\1 \2 \3/p' "$install")
    if [ -z "$rows" ]; then
        pin_fail 'install.sh does not pin release archives'
        return 1
    fi
    printf '%s\n' "version $version"
    seen=' '
    printf '%s\n' "$rows" | while read -r mode target digest; do
        case $mode in
            client) name="kiln-v${PIN_VERSION}-${target}.tar.gz" ;;
            server) name="kiln-server-v${PIN_VERSION}-${target}.tar.gz" ;;
            *) pin_fail 'archive mode'; exit 1 ;;
        esac
        case $name in
            *[!A-Za-z0-9._-]*) pin_fail "archive name $name"; exit 1 ;;
        esac
        case $seen in
            *" $name "*) pin_fail "archive name $name"; exit 1 ;;
        esac
        seen="$seen$name "
        printf '%s %s\n' "$name" "$digest"
    done
}

pin_fetch() {
    version=$1
    name=$2
    dest=$3
    if [ -n "${PIN_FETCH_CMD:-}" ]; then
        "$PIN_FETCH_CMD" "$version" "$name" "$dest"
        return
    fi
    url="https://github.com/${PIN_REPO}/releases/download/v${version}/${name}"
    curl -fsSL --retry 3 --proto '=https' --proto-redir '=https' \
        --connect-timeout 30 --max-time 900 \
        -o "$dest" "$url"
}

pin_verify() {
    archive=$1
    err=$(mktemp)
    set +e
    gh attestation verify "$archive" \
        --repo "$PIN_REPO" \
        --cert-identity-regex "$PIN_IDENTITY" >"$err" 2>&1
    code=$?
    set -e
    if [ "$code" -ne 0 ]; then
        detail=$(tail -n 1 "$err" || true)
        token=${GH_TOKEN:-}
        if [ -n "$token" ]; then
            detail=$(printf '%s\n' "$detail" | sed "s|$token||g")
        fi
        token=${GITHUB_TOKEN:-}
        if [ -n "$token" ]; then
            detail=$(printf '%s\n' "$detail" | sed "s|$token||g")
        fi
        printf 'attestation failed for %s: %s\n' "$(basename "$archive")" "$detail" >&2
        rm -f "$err"
        return 1
    fi
    rm -f "$err"
}

pin_check() {
    install=$1
    docs=$2
    pin_parse_docs "$docs"
    work=$(mktemp -d)
    if ! pin_parse_install "$install" >"$work/parsed"; then
        rm -rf "$work"
        return 1
    fi
    PIN_VERSION=$(awk 'NR == 1 { print $2; exit }' "$work/parsed")
    if [ -z "$PIN_VERSION" ]; then
        pin_fail 'install.sh must pin one release version'
        rm -rf "$work"
        return 1
    fi
    awk 'NR > 1 { print }' "$work/parsed" >"$work/rows"
    while read -r name digest; do
        [ -n "$name" ] || continue
        case $name in
            *[!A-Za-z0-9._-]*) pin_fail "archive name $name"; rm -rf "$work"; return 1 ;;
        esac
        dest=$work/$name
        pin_fetch "$PIN_VERSION" "$name" "$dest"
        actual=$(pin_sha256 "$dest")
        if [ "$actual" != "$digest" ]; then
            printf 'kiln: %s sha256 is %s\n' "$name" "$actual" >&2
            rm -rf "$work"
            return 1
        fi
        if ! pin_verify "$dest"; then
            rm -rf "$work"
            return 1
        fi
    done <"$work/rows"
    rm -rf "$work"
}

main() {
    root=$(dirname "$0")
    root=$(cd "$root/.." && pwd)
    pin_check "$root/deploy/bootstrap/install.sh" "$root/docs/releases.md"
}

case ${0##*/} in
    check-bootstrap-pins.sh) main "$@" ;;
esac
