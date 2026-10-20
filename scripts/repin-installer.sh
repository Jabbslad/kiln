#!/bin/sh
# Move the public installer pin to one published tag and open a pull request.
set -eu

tag_ok() {
    printf '%s\n' "$1" | awk 'BEGIN { found = 0 } /^v[0-9]+\.[0-9]+\.[0-9]+$/ { found = 1 } END { exit !found }'
}

read_digest() {
    name=$1
    tag=$2
    cleanup=
    if [ -n "${REPIN_SHA_DIR:-}" ]; then
        file=${REPIN_SHA_DIR%/}/$name.sha256
        if [ ! -f "$file" ]; then
            printf 'repin: missing checksum %s\n' "$name" >&2
            return 1
        fi
    else
        file=$(mktemp)
        cleanup=$file
        url="https://github.com/${GH_REPO:-Jabbslad/kiln}/releases/download/${tag}/${name}.sha256"
        if ! curl -fsSL --retry 3 --proto '=https' --proto-redir '=https' \
            --connect-timeout 30 --max-time 60 \
            -o "$file" "$url"; then
            rm -f "$cleanup"
            printf 'repin: checksum download failed for %s\n' "$name" >&2
            return 1
        fi
    fi
    got=$(awk 'NR == 1 { print $1; exit }' "$file")
    fname=$(awk 'NR == 1 { print $2; exit }' "$file")
    if [ -n "$cleanup" ]; then
        rm -f "$cleanup"
    fi
    if [ "$fname" != "$name" ]; then
        printf 'repin: checksum name for %s was %s\n' "$name" "$fname" >&2
        return 1
    fi
    printf '%s\n' "$got" | awk 'BEGIN { ok = 0 } length($0) == 64 && $0 ~ /^[0-9a-f]+$/ { ok = 1 } END { exit !ok }' || {
        printf 'repin: checksum for %s is not a sha256\n' "$name" >&2
        return 1
    }
    printf '%s\n' "$got"
}

rewrite_file() {
    src=$1
    tmp=$(mktemp)
    if ! awk -v version="$2" -v linux="$3" -v intel="$4" -v arm="$5" -v server="$6" -v kind="$7" '
        function hex64(value) {
            return length(value) == 64 && value ~ /^[0-9a-f]+$/
        }
        BEGIN { bad = 0; hits = 0 }
        kind == "install" && /^    version=[0-9]+\.[0-9]+\.[0-9]+$/ {
            if (!hex64(linux) || !hex64(intel) || !hex64(arm) || !hex64(server)) bad = 1
            print "    version=" version
            hits++
            next
        }
        kind == "install" && /^client:x86_64-unknown-linux-gnu [0-9a-f]+$/ {
            print "client:x86_64-unknown-linux-gnu " linux
            hits++
            next
        }
        kind == "install" && /^client:x86_64-apple-darwin [0-9a-f]+$/ {
            print "client:x86_64-apple-darwin " intel
            hits++
            next
        }
        kind == "install" && /^client:aarch64-apple-darwin [0-9a-f]+$/ {
            print "client:aarch64-apple-darwin " arm
            hits++
            next
        }
        kind == "install" && /^server:x86_64-unknown-linux-gnu [0-9a-f]+$/ {
            print "server:x86_64-unknown-linux-gnu " server
            hits++
            next
        }
        kind == "docs" && /^The published bootstrap pin is v[0-9]+\.[0-9]+\.[0-9]+\.$/ {
            print "The published bootstrap pin is v" version "."
            hits++
            next
        }
        { print }
        END {
            if (kind == "install" && (bad || hits != 5)) exit 1
            if (kind == "docs" && hits != 1) exit 1
        }
    ' "$src" >"$tmp"; then
        rm -f "$tmp"
        printf 'repin: %s pin was not rewritten\n' "$src" >&2
        return 1
    fi
    cat "$tmp" >"$src"
    rm -f "$tmp"
}

pin_files() {
    tag=$1
    root=$2
    if ! tag_ok "$tag"; then
        printf 'repin: tag must be vMAJOR.MINOR.PATCH\n' >&2
        return 1
    fi
    version=${tag#v}
    linux=$(read_digest "kiln-v${version}-x86_64-unknown-linux-gnu.tar.gz" "$tag")
    intel=$(read_digest "kiln-v${version}-x86_64-apple-darwin.tar.gz" "$tag")
    arm=$(read_digest "kiln-v${version}-aarch64-apple-darwin.tar.gz" "$tag")
    server=$(read_digest "kiln-server-v${version}-x86_64-unknown-linux-gnu.tar.gz" "$tag")
    rewrite_file "$root/deploy/bootstrap/install.sh" "$version" "$linux" "$intel" "$arm" "$server" install
    rewrite_file "$root/docs/releases.md" "$version" "$linux" "$intel" "$arm" "$server" docs
}

if [ "${1:-}" = pin ]; then
    pin_files "${2:?}" "${3:?}"
    exit 0
fi

if [ -z "${REPIN_EXEC:-}" ]; then
    REPIN_ROOT=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
    dest=$(mktemp)
    cp "$0" "$dest"
    REPIN_EXEC=1
    export REPIN_ROOT REPIN_EXEC
    exec sh "$dest"
fi

if [ -z "${REPIN_TOKEN:-}" ]; then
    printf '%s\n' 'REPIN_TOKEN is unset' >&2
    exit 1
fi

tag=${TAG:?}
repo=${GH_REPO:?}
if ! tag_ok "$tag"; then
    printf 'repin: tag must be vMAJOR.MINOR.PATCH\n' >&2
    exit 1
fi
branch=repin/$tag
case $branch in
    repin/v*) ;;
    *)
        printf 'repin: refusing to push\n' >&2
        exit 1
        ;;
esac

export GH_TOKEN="$REPIN_TOKEN"
cd "$REPIN_ROOT" || exit 1
git fetch origin main
git checkout -B "$branch" origin/main
pin_files "$tag" "$REPIN_ROOT"
git add -- deploy/bootstrap/install.sh docs/releases.md
if git diff --cached --quiet; then
    printf 'pin already matches %s\n' "$tag"
    exit 0
fi
git config user.name kiln-repin
git config user.email kiln-repin@users.noreply.github.com
git commit -m "chore(bootstrap): pin the public installer to ${tag}"
gh auth setup-git
git push origin "HEAD:repin/${tag}"
number=$(gh pr list --repo "$repo" --head "$branch" --base main --state open --json number --jq '.[0].number // empty')
if [ -n "$number" ]; then
    exit 0
fi
body=$(printf '%s\n\n%s\n' \
    "The public installer pin now tracks ${tag}." \
    "deploy/bootstrap/install.sh and docs/releases.md carry that release's version and archive digests.")
gh pr create --repo "$repo" --base main --head "$branch" \
    --title "chore(bootstrap): pin the public installer to ${tag}" \
    --body "$body"
