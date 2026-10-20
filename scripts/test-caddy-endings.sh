#!/bin/sh
set -eu

root=$(dirname "$0")
root=$(cd "$root/.." && pwd)
runbook=$root/docs/runbooks/2026-10-20-app-dark-forge-caddy.md
site=$root/deploy/app.Caddyfile

fail() {
    printf 'test-caddy-endings: %s\n' "$*" >&2
    exit 1
}

command -v caddy >/dev/null 2>&1 || fail 'caddy is not installed'

if grep -q "$(printf '\r')" "$site"; then
    fail 'site fixture contains CR'
fi

block=$(awk '
    /^```sh$/ { capture = 1; buf = ""; next }
    /^```$/ && capture {
        if (index(buf, "split_at")) {
            printf "%s", buf
            found = 1
        }
        capture = 0
        next
    }
    capture { buf = buf $0 ORS }
    END { if (!found) exit 1 }
' "$runbook") || fail 'repair block not found'

block=$(printf '%s\n' "$block" | sed \
    -e "s|/opt/paseo/Caddyfile|\"\$caddyfile\"|g" \
    -e "s|deploy/app.Caddyfile|\"\$site\"|g")

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
body=$tmp/body
tail -n +2 "$site" >"$body"

append_site() {
    live=$1
    {
        printf '%s\n' "caddyfile=\$1" "site=\$2"
        printf '%s\n' "$block"
    } >"$tmp/append.sh"
    sh "$tmp/append.sh" "$live" "$site"
}

has_bare_lf() {
    awk '{ if (!sub(/\r$/, "", $0)) found = 1 } END { exit !found }' "$1"
}

cmp_prefix() {
    file=$1
    prefix=$2
    size=$(wc -c <"$prefix" | awk '{ print $1 }')
    head -c "$size" "$file" | cmp -s - "$prefix"
}

cmp_suffix() {
    file=$1
    suffix=$2
    size=$(wc -c <"$suffix" | awk '{ print $1 }')
    tail -c "$size" "$file" | cmp -s - "$suffix"
}

present=$tmp/present
{
    printf '# keep this line\n'
    printf 'example.test {\r\n\trespond "ok"\r\n}\r\n'
    printf 'app.dark-forge.dev {\n'
    cat "$body"
} >"$present"
cp "$present" "$tmp/present-original"
has_bare_lf "$present" || fail 'fixture has no bare LF'
grep -q "$(printf '\r')" "$present" || fail 'fixture has no CR'
append_site "$present"
append_site "$present"
cmp -s "$present" "$tmp/present-original" || fail 'mixed file with the site was rewritten'

glued=$tmp/glued
{
    printf 'example.test {\r\n\trespond "ok"\r\n'
    printf '}'
    printf 'app.dark-forge.dev {\n'
    cat "$body"
} >"$glued"
append_site "$glued"
cp "$glued" "$tmp/glued-once"
append_site "$glued"
cmp -s "$glued" "$tmp/glued-once" || fail 'second repair changed the file'
{
    printf 'example.test {\r\n\trespond "ok"\r\n'
    printf '}\n'
    printf 'app.dark-forge.dev {\n'
} >"$tmp/glued-prefix"
cmp_prefix "$glued" "$tmp/glued-prefix" || fail 'glued line did not keep its ending'
cmp_suffix "$glued" "$body" || fail 'site body changed'
has_bare_lf "$glued" || fail 'glued repair erased LF lines'
count=$(grep -a -c 'app.dark-forge.dev {' "$glued" || true)
[ "$count" -eq 1 ] || fail "site count $count"
caddy validate --config "$glued" --adapter caddyfile >/dev/null

appended=$tmp/appended
{
    printf '# keep this line\n'
    printf 'example.test {\r\n\trespond "ok"\r\n}\r\n'
} >"$appended"
append_site "$appended"
cp "$appended" "$tmp/appended-once"
append_site "$appended"
cmp -s "$appended" "$tmp/appended-once" || fail 'second append changed the file'
{
    printf '# keep this line\n'
    printf 'example.test {\r\n\trespond "ok"\r\n}\r\n'
} >"$tmp/appended-prefix"
cmp_prefix "$appended" "$tmp/appended-prefix" || fail 'existing lines changed'
grep -a -q "$(printf '\r\n}\r\napp.dark-forge.dev {\r\n')" "$appended" || fail 'appended site was not CRLF'
if grep -a -q "$(printf '# keep this line\r\n')" "$appended"; then
    fail 'LF note was rewritten as CRLF'
fi

printf '%s\n' 'ok'
