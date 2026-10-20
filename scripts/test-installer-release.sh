#!/bin/sh
# The published v0.4.14 installer reports 0.4.10 and downloads v0.4.14.
# This runs that asset, read-only, and requires those versions to match the tag.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tag=v0.4.14
version=0.4.14
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL --proto '=https' --proto-redir '=https' \
    --connect-timeout 30 --max-time 60 \
    -o "$work/install.sh" \
    "https://github.com/Jabbslad/kiln/releases/download/${tag}/install.sh"

# Stub the host so the asset prints its version and curl URL, then stops.
# Nothing is installed.
mkdir -p "$work/bin" "$work/home" "$work/tmp"
cat >"$work/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in
    -s) printf '%s\n' Linux ;;
    -m) printf '%s\n' x86_64 ;;
esac
EOF
cat >"$work/bin/getconf" <<'EOF'
#!/bin/sh
printf '%s\n' 'glibc 2.39'
EOF
cat >"$work/bin/curl" <<EOF
#!/bin/sh
printf '%s\n' "\$*" > '$work/curl-args'
exit 1
EOF
chmod 755 "$work/bin/uname" "$work/bin/getconf" "$work/bin/curl"

set +e
env -u KILN_RELEASE -u FIXTURE \
    HOME="$work/home" TMPDIR="$work/tmp" PATH="$work/bin:$PATH" \
    sh "$work/install.sh" >"$work/out" 2>"$work/err"
set -e

reported=$(sed -n 's/^Downloading kiln \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\) .*/\1/p' "$work/out")
url=$(awk '{ print $NF }' "$work/curl-args")
case $url in
    https://github.com/Jabbslad/kiln/releases/download/v[0-9]*.[0-9]*.[0-9]*/*) ;;
    *)
        printf 'test-installer-release: download URL was %s\n' "$url" >&2
        exit 1
        ;;
esac
downloaded=${url#https://github.com/Jabbslad/kiln/releases/download/v}
downloaded=${downloaded%%/*}

if [ "$reported" != "$version" ] || [ "$downloaded" != "$version" ] || [ "$reported" != "$downloaded" ]; then
    printf 'test-installer-release: reported %s but downloaded %s from %s\n' \
        "$reported" "$downloaded" "$tag" >&2
    exit 1
fi

printf '%s\n' ok
