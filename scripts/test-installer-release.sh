#!/bin/sh
# Read-only check of the published v0.4.14 installer and of this tree's resolver.
# The asset assigns version=0.4.10 while its package URL is v0.4.14.
# Aligning sets version from that URL. Nothing is installed.
set -eu

root=$(dirname "$0")
root=$(cd "$root/.." && pwd)
tag=v0.4.14
version=0.4.14
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
    printf 'test-installer-release: %s\n' "$*" >&2
    exit 1
}

curl -fsSL --proto '=https' --proto-redir '=https' \
    --connect-timeout 30 --max-time 60 \
    -o "$work/published.sh" \
    "https://github.com/Jabbslad/kiln/releases/download/${tag}/install.sh"

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

capture() {
    script=$1
    rm -rf "${work:?}/home" "${work:?}/tmp"
    mkdir -p "$work/home" "$work/tmp"
    rm -f "$work/curl-args" "$work/out" "$work/err"
    set +e
    env -u KILN_RELEASE -u FIXTURE -u KILN_RESOLVE_ONLY \
        HOME="$work/home" TMPDIR="$work/tmp" PATH="$work/bin:$PATH" \
        sh "$script" >"$work/out" 2>"$work/err"
    set -e
    if [ -e "$work/home/.local/bin/kiln" ]; then
        fail "$script installed a client"
    fi
    reported=$(sed -n 's/^Downloading kiln \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\) .*/\1/p' "$work/out")
    url=$(awk '{ print $NF }' "$work/curl-args")
    case $url in
        https://github.com/Jabbslad/kiln/releases/download/v[0-9]*.[0-9]*.[0-9]*/*) ;;
        *) fail "download URL was $url" ;;
    esac
    downloaded=${url#https://github.com/Jabbslad/kiln/releases/download/v}
    downloaded=${downloaded%%/*}
}

capture "$work/published.sh"
[ "$reported" = 0.4.10 ] || fail "published asset reported $reported"
[ "$downloaded" = "$version" ] || fail "published asset downloaded $downloaded"

cp "$work/published.sh" "$work/aligned.sh"
sh "$root/deploy/bootstrap/install.sh" --align-release "$work/aligned.sh"
capture "$work/aligned.sh"
[ "$reported" = "$version" ] || fail "aligned installer reported $reported"
[ "$downloaded" = "$version" ] || fail "aligned installer downloaded $downloaded"
grep -q "^    version=${version}$" "$work/aligned.sh" || fail 'aligned version line'

sed -n '/^# BEGIN RELEASE ASSETS$/,/^# END RELEASE ASSETS$/p' "$work/published.sh" >"$work/assets"
seen=0
while read -r key digest; do
    case $key in
        client:*) name="kiln-v${version}-${key#client:}.tar.gz" ;;
        server:*) name="kiln-server-v${version}-${key#server:}.tar.gz" ;;
        *) continue ;;
    esac
    case $digest in
        [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;
        *) continue ;;
    esac
    sidecar=$(curl -fsSL --proto '=https' --proto-redir '=https' \
        --connect-timeout 30 --max-time 60 \
        "https://github.com/Jabbslad/kiln/releases/download/${tag}/${name}.sha256")
    got=$(printf '%s\n' "$sidecar" | awk 'NR == 1 { print $1 }')
    [ "$got" = "$digest" ] || fail "$name sidecar is $got"
    seen=$((seen + 1))
done <"$work/assets"
[ "$seen" -eq 4 ] || fail "expected 4 checksums, got $seen"

resolved=$(KILN_RESOLVE_ONLY=1 KILN_RELEASE="$tag" sh "$root/deploy/bootstrap/install.sh")
printf '%s\n' "$resolved" >"$work/resolved"
count=$(grep -c . "$work/resolved" || true)
[ "$count" -eq 4 ] || fail "resolver printed $count lines"
while read -r got_version spec got_digest; do
    [ "$got_version" = "$version" ] || fail "resolver version $got_version"
    case $spec in
        client:*) name="kiln-v${version}-${spec#client:}.tar.gz" ;;
        server:*) name="kiln-server-v${version}-${spec#server:}.tar.gz" ;;
        *) fail "resolver spec $spec" ;;
    esac
    sidecar=$(curl -fsSL --proto '=https' --proto-redir '=https' \
        --connect-timeout 30 --max-time 60 \
        "https://github.com/Jabbslad/kiln/releases/download/${tag}/${name}.sha256")
    want=$(printf '%s\n' "$sidecar" | awk 'NR == 1 { print $1 }')
    [ "$got_digest" = "$want" ] || fail "$name resolved $got_digest"
done <"$work/resolved"

workflow=$root/.github/workflows/build.yml
if grep -n 'REPIN_TOKEN' "$workflow"; then
    fail 'build.yml names a repin token'
fi
if grep -n '^  repin:' "$workflow"; then
    fail 'build.yml still has a repin job'
fi
grep -q -- '--align-release' "$workflow" || fail 'publish does not set version from the tag'
if [ -e "$root/scripts/repin-installer.sh" ]; then
    fail 'repin script remains'
fi
if grep -R -n --exclude="$(basename "$0")" 'REPIN_TOKEN' \
    "$root/.github" "$root/scripts" "$root/crates" "$root/deploy" "$root/docs"
then
    fail 'REPIN_TOKEN remains'
fi

printf '%s\n' ok
