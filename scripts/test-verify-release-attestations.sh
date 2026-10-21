#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
verify=$root/scripts/verify-release-attestations.sh

fail() {
  printf 'test-verify-release-attestations: %s\n' "$*" >&2
  exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/dist" "$tmp/one"
printf 'x' >"$tmp/dist/a.tar.gz"
printf 'y' >"$tmp/dist/b.tar.gz"
printf 'x' >"$tmp/one/a.tar.gz"

cat >"$tmp/gh" <<'EOF'
#!/bin/sh
set -eu
log=${GH_ATTEST_LOG:?}
mode=${GH_ATTEST_MODE:?}
state=${GH_ATTEST_STATE:?}
printf '%s\n' "$*" >>"$log"
case "$1" in
  attestation)
    [ "${2:-}" = verify ] || exit 1
    ;;
  *) exit 1 ;;
esac
n=0
if [ -f "$state" ]; then
  n=$(tr -d '[:space:]' <"$state")
fi
case $n in
  ''|*[!0-9]*) n=0 ;;
esac
n=$((n + 1))
printf '%s\n' "$n" >"$state"
case $mode in
  transient-once)
    if [ "$n" -eq 1 ]; then
      printf 'Error: failed to choose verifier based on provided bundle issuer: public good verifier is not available (initialization may have failed)\n' >&2
      exit 1
    fi
    exit 0
    ;;
  transient-always)
    printf 'Error: failed to choose verifier based on provided bundle issuer: public good verifier is not available (initialization may have failed)\n' >&2
    exit 1
    ;;
  mismatch)
    printf 'Error: attestation verification failed: certificate identity did not match\n' >&2
    exit 1
    ;;
  *) exit 1 ;;
esac
EOF
chmod 755 "$tmp/gh"

run_verify() {
  mode=$1
  expect=$2
  distdir=$3
  want_calls=${4:-}
  : >"$tmp/state"
  : >"$tmp/log"
  : >"$tmp/err"
  set +e
  GH_ATTEST_MODE=$mode \
    GH_ATTEST_LOG=$tmp/log \
    GH_ATTEST_STATE=$tmp/state \
    KILN_ATTEST_SOURCE_REF=refs/heads/main \
    PATH="$tmp:$PATH" \
    sh "$verify" "$distdir" >"$tmp/out" 2>"$tmp/err"
  code=$?
  set -e
  if [ "$code" -ne "$expect" ]; then
    fail "$mode: expected exit $expect got $code stderr=$(cat "$tmp/err")"
  fi
  if [ -n "$want_calls" ]; then
    calls=$(wc -l <"$tmp/log" | tr -d ' ')
    [ "$calls" -eq "$want_calls" ] || fail "$mode: expected $want_calls gh calls, got $calls"
  fi
}

run_verify transient-once 0 "$tmp/one" 2

run_verify transient-always 1 "$tmp/one" 3
grep -q 'archive=' "$tmp/err" || fail 'transient-always: missing archive on failure'
grep -q 'command=' "$tmp/err" || fail 'transient-always: missing command on failure'
grep -q 'exit=' "$tmp/err" || fail 'transient-always: missing exit on failure'
grep -q 'output=' "$tmp/err" || fail 'transient-always: missing output on failure'

run_verify mismatch 1 "$tmp/one" 1
grep -q 'certificate identity did not match' "$tmp/err" || fail 'mismatch: missing verification output'

# Orphan tag v0.4.46: next planned tag is v0.4.47, not a collision.
tags=$tmp/orphan-tags.txt
{
  printf '%s\n' 'refs/tags/v0.4.45'
  printf '%s\n' 'refs/tags/v0.4.46'
} >"$tags"
next=$(sh "$root/scripts/next-release-tag.sh" <"$tags")
[ "$next" = v0.4.47 ] || fail "orphan tag: expected v0.4.47 got $next"
sh "$root/scripts/next-release-tag.sh" --check v0.4.47 <"$tags"

printf 'verify release attestations passed\n'
