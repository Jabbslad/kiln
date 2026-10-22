#!/bin/sh
set -eu

root=$(dirname "$0")
root=$(cd "$root/.." && pwd)
. "$root/scripts/check-bootstrap-pins.sh"

fail() {
    printf 'test-bootstrap-pins: %s\n' "$*" >&2
    exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

docs=$root/docs/releases.md
install=$root/deploy/bootstrap/install.sh
pin_parse_docs "$docs"
parsed=$(pin_parse_install "$install")
PIN_VERSION=$(printf '%s\n' "$parsed" | awk 'NR == 1 { print $2 }')
count=$(printf '%s\n' "$parsed" | awk 'NR > 1' | grep -c .)
[ "$count" -eq 4 ] || fail "expected 4 archives, got $count"
if grep -q '^The published bootstrap pin is v' "$docs"; then
    fail 'docs still describe a committed pin'
fi
grep -q '^The installer version is the release tag\.$' "$docs" || fail 'release tag sentence'

base='https://github.com/Jabbslad/kiln/.github/workflows/build.yml@'
accept() {
    printf '%s\n' "$1" | grep -E -q "$PIN_IDENTITY" || fail "should accept $1"
}
reject() {
    if printf '%s\n' "$1" | grep -E -q "$PIN_IDENTITY"; then
        fail "should reject $1"
    fi
}
accept "${base}refs/heads/main"
accept "${base}refs/tags/v${PIN_VERSION}"
reject "${base}refs/heads/feature"
reject "${base}refs/tags/v0.4"
reject "${base}refs/heads/main-backup"

bad_install=$tmp/two-versions
{
    printf '    version=1.2.3\n'
    printf '    version=1.2.4\n'
    printf 'client:x86_64-unknown-linux-gnu %s\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
} >"$bad_install"
if pin_parse_install "$bad_install" >/dev/null 2>"$tmp/err"; then
    fail 'two versions were accepted'
fi
grep -q 'one release version' "$tmp/err" || fail 'two versions missing message'

slash_install=$tmp/slash
{
    printf '    version=1.2.3\n'
    printf 'client:../secret %s\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
} >"$slash_install"
if pin_parse_install "$slash_install" >/dev/null 2>"$tmp/err"; then
    fail 'slash target was accepted'
fi

empty_install=$tmp/empty
printf '    version=1.2.3\n' >"$empty_install"
if pin_parse_install "$empty_install" >/dev/null 2>"$tmp/err"; then
    fail 'missing archives were accepted'
fi

blob='archive-bytes'
printf '%s' "$blob" >"$tmp/blob"
digest=$(pin_sha256 "$tmp/blob")
name="kiln-v1.2.3-x86_64-unknown-linux-gnu.tar.gz"
good_install=$tmp/good-install
{
    printf '    version=1.2.3\n'
    printf 'client:x86_64-unknown-linux-gnu %s\n' "$digest"
} >"$good_install"
good_docs=$tmp/good-docs
printf '%s\n' "verify with \`gh attestation verify ARCHIVE --repo Jabbslad/kiln --cert-identity-regex '^https://github\\.com/Jabbslad/kiln/\\.github/workflows/build\\.yml@refs/(heads/main|tags/v[0-9]+\\.[0-9]+\\.[0-9]+)\$'\`" >"$good_docs"

fetch_ok=$tmp/fetch-ok
cat >"$fetch_ok" <<EOF
#!/bin/sh
printf '%s' '$blob' > "\$3"
EOF
chmod +x "$fetch_ok"

called=$tmp/gh-called
fetch_bad=$tmp/fetch-bad
cat >"$fetch_bad" <<EOF
#!/bin/sh
printf '%s' 'other-bytes' > "\$3"
EOF
chmod +x "$fetch_bad"

gh=$tmp/bin/gh
mkdir -p "$tmp/bin"
cat >"$gh" <<EOF
#!/bin/sh
printf '%s\\n' "\$@" > '$tmp/argv'
touch '$called'
exit 1
EOF
chmod +x "$gh"

PIN_FETCH_CMD=$fetch_bad
PATH="$tmp/bin:$PATH"
export PATH PIN_FETCH_CMD
rm -f "$called"
if pin_check "$good_install" "$good_docs" >"$tmp/out" 2>"$tmp/err"; then
    fail 'wrong digest was accepted'
fi
grep -q "$name sha256 is" "$tmp/err" || fail 'digest error missing'
if [ -e "$called" ]; then
    fail 'attested a mismatched archive'
fi

PIN_FETCH_CMD=$fetch_ok
export PIN_FETCH_CMD
GH_TOKEN='super-secret-token'
export GH_TOKEN
if pin_check "$good_install" "$good_docs" >"$tmp/out" 2>"$tmp/err"; then
    fail 'failed attestation was accepted'
fi
grep -q "attestation failed for $name" "$tmp/err" || fail 'attestation error missing'
if grep -q "$GH_TOKEN" "$tmp/err"; then
    fail 'token leaked'
fi
grep -q -- '--cert-identity-regex' "$tmp/argv" || fail 'verify command missing regex'
grep -q 'Jabbslad/kiln' "$tmp/argv" || fail 'verify command missing repo'

cat >"$gh" <<EOF
#!/bin/sh
exit 0
EOF
chmod +x "$gh"
pin_check "$good_install" "$good_docs" >"$tmp/out" 2>"$tmp/err" || fail 'matching pin was rejected'

workflow=$root/.github/workflows/bootstrap-pins.yml
build=$root/.github/workflows/build.yml
grep -q 'deploy/bootstrap/install.sh' "$workflow" || fail 'workflow paths'
grep -q 'docs/releases.md' "$workflow" || fail 'workflow paths'
grep -q 'scripts/check-bootstrap-pins.sh' "$workflow" || fail 'workflow paths'
grep -q 'schedule:' "$workflow" || fail 'workflow schedule'
grep -q "cron: '" "$workflow" || fail 'workflow cron'
grep -q 'sh scripts/check-bootstrap-pins.sh' "$workflow" || fail 'workflow command'
grep -q 'attestations: read' "$workflow" || fail 'workflow permission'
grep -q 'contents: read' "$workflow" || fail 'workflow permission'
grep -q 'persist-credentials: false' "$workflow" || fail 'workflow checkout'
needle=$(printf '%s%s%s' 'GH_TOKEN: ' '$' '{{ github.token }}')
grep -q -F "$needle" "$workflow" || fail 'workflow token'
if grep -q 'pull_request_target' "$workflow"; then
    fail 'workflow uses pull_request_target'
fi
if grep -q 'contents: write' "$workflow"; then
    fail 'workflow can write contents'
fi
if grep -q 'check_bootstrap_pins.py' "$workflow" "$build"; then
    fail 'workflow still calls python'
fi
lint=$root/scripts/server-check-test-and-lint.sh
n=$(grep -h '^[[:space:]]*sh scripts/test-bootstrap-pins.sh$' "$build" "$lint" | wc -l | tr -d ' ')
[ "$n" -eq 2 ] || fail "build runs the pin test $n times"
n=$(grep -h '^[[:space:]]*sh scripts/test-caddy-endings.sh$' "$build" "$lint" | wc -l | tr -d ' ')
[ "$n" -eq 2 ] || fail "build runs the ending test $n times"

printf '%s\n' 'ok'
