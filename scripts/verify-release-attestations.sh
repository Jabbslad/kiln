#!/bin/sh
# Verify release archive attestations. Retries only Sigstore public-good verifier init flakes.
set -eu

fail() {
  printf 'verify-release-attestations: %s\n' "$*" >&2
  exit 1
}

report_failure() {
  archive=$1
  cmd=$2
  code=$3
  output=$4
  printf 'verify-release-attestations: archive=%s\n' "$archive" >&2
  printf 'verify-release-attestations: command=%s\n' "$cmd" >&2
  printf 'verify-release-attestations: exit=%s\n' "$code" >&2
  printf 'verify-release-attestations: output=%s\n' "$output" >&2
}

verify_archive() {
  archive=$1
  source_ref=$2
  regex=$3
  repo=$4
  transient='public good verifier is not available (initialization may have failed)'
  cmd="gh attestation verify \"$archive\" --repo $repo --cert-identity-regex \"$regex\" --source-ref \"$source_ref\""
  attempt=1
  while [ "$attempt" -le 3 ]; do
    err=$(mktemp)
    set +e
    gh attestation verify "$archive" \
      --repo "$repo" \
      --cert-identity-regex "$regex" \
      --source-ref "$source_ref" >"$err" 2>&1
    code=$?
    set -e
    output=$(cat "$err")
    rm -f "$err"
    if [ "$code" -eq 0 ]; then
      return 0
    fi
    if printf '%s\n' "$output" | grep -Fq "$transient"; then
      if [ "$attempt" -lt 3 ]; then
        case $attempt in
          1) sleep 5 ;;
          2) sleep 15 ;;
        esac
        attempt=$((attempt + 1))
        continue
      fi
    else
      report_failure "$archive" "$cmd" "$code" "$output"
      return 1
    fi
    report_failure "$archive" "$cmd" "$code" "$output"
    return 1
  done
}

dist=${1:-dist}
source_ref=${KILN_ATTEST_SOURCE_REF:-refs/heads/main}
repo=${KILN_ATTEST_REPO:-Jabbslad/kiln}
regex='^https://github\.com/Jabbslad/kiln/\.github/workflows/build\.yml@refs/(heads/main|tags/v[0-9]+\.[0-9]+\.[0-9]+)$'

[ -d "$dist" ] || fail "missing dist directory ${dist}"

found=0
pids=
for archive in "$dist"/*.tar.gz; do
  [ -f "$archive" ] || continue
  found=1
  verify_archive "$archive" "$source_ref" "$regex" "$repo" &
  pids="$pids $!"
done
[ "$found" = 1 ] || fail "no archives in ${dist}"

fail=0
for pid in $pids; do
  wait "$pid" || fail=1
done
[ "$fail" -eq 0 ] || exit 1
