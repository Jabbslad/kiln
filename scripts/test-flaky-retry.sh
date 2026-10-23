#!/bin/sh
# One automatic retry for listed flaky tests, none for the rest, with a count.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
runner=$root/scripts/run-workspace-nextest.sh
list=$root/scripts/flaky-tests.txt
listed_pkg=kiln-runtime
listed_test=generation_disk_relocate_beats_copy_for_storage_ms
unlisted_test=unlisted_test_should_not_retry

fail() {
  printf 'test-flaky-retry: %s\n' "$*" >&2
  exit 1
}

[ -x "$runner" ] || fail 'missing run-workspace-nextest.sh'
[ -f "$list" ] || fail 'missing scripts/flaky-tests.txt'
grep -q "^${listed_pkg} ${listed_test}$" "$list" \
  || fail 'flaky list must name kiln-runtime generation_disk_relocate_beats_copy_for_storage_ms'

expected=0
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    '' | \#*) continue ;;
  esac
  expected=$((expected + 1))
done <"$list"
[ "$expected" -ge 1 ] || fail 'flaky list has no tests'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cat >"$work/cargo" <<'EOF'
#!/bin/sh
set -eu
dir=${KILN_FLAKY_STUB_DIR:?}
printf '%s\n' "$*" >>"$dir/invocations"
cfg=
prev=
for arg in "$@"; do
  if [ "$prev" = "--config-file" ]; then
    cfg=$arg
  fi
  prev=$arg
done
if [ -z "$cfg" ] || [ ! -f "$cfg" ]; then
  printf 'stub: cargo nextest must receive --config-file\n' >&2
  exit 2
fi
cp "$cfg" "$dir/seen-config"
if ! grep -qx 'retries = 0' "$cfg"; then
  printf 'stub: default retries must be 0\n' >&2
  exit 2
fi
if ! grep -qx 'retries = 1' "$cfg"; then
  printf 'stub: listed tests must set retries = 1\n' >&2
  exit 2
fi
bad=$(grep -E '^retries = ' "$cfg" | grep -v -E '^retries = (0|1)$' || true)
if [ -n "$bad" ]; then
  printf 'stub: unexpected retry count: %s\n' "$bad" >&2
  exit 2
fi
ones=$(grep -cx 'retries = 1' "$cfg" || true)
want=${KILN_FLAKY_EXPECT:-1}
if [ "$ones" -ne "$want" ]; then
  printf 'stub: expected %s retries = 1 lines, got %s\n' "$want" "$ones" >&2
  exit 2
fi
if ! grep -F -q "test(${KILN_FLAKY_LISTED_TEST:?})" "$cfg"; then
  printf 'stub: config missing listed test filter\n' >&2
  exit 2
fi
if ! grep -F -q "package(${KILN_FLAKY_LISTED_PKG:?})" "$cfg"; then
  printf 'stub: config missing listed package filter\n' >&2
  exit 2
fi
if grep -F -q "${KILN_FLAKY_UNLISTED_TEST:?}" "$cfg"; then
  printf 'stub: unlisted test must not appear in the retry config\n' >&2
  exit 2
fi
workspace=0
for arg in "$@"; do
  if [ "$arg" = "--workspace" ]; then
    workspace=1
  fi
done
if [ "$workspace" -ne 1 ]; then
  exit 0
fi
case "${KILN_FLAKY_CASE:?}" in
  retry-pass | retry-fail)
    printf '       RETRY 2/2 [         ] %s::box_start_storage %s\n' \
      "$KILN_FLAKY_LISTED_PKG" "$KILN_FLAKY_LISTED_TEST"
    if [ "$KILN_FLAKY_CASE" = retry-fail ]; then
      exit 1
    fi
    exit 0
    ;;
  unlisted-fail)
    printf '        FAIL [   0.001s] %s::box_start_storage %s\n' \
      "$KILN_FLAKY_LISTED_PKG" "$KILN_FLAKY_UNLISTED_TEST"
    exit 1
    ;;
  *)
    printf 'stub: unknown case %s\n' "$KILN_FLAKY_CASE" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$work/cargo"

run_case() {
  case_name=$1
  : >"$work/invocations"
  rm -f "$work/seen-config" "$root/target/flaky-retries.log"
  set +e
  KILN_FLAKY_CASE=$case_name \
    KILN_FLAKY_STUB_DIR=$work \
    KILN_FLAKY_EXPECT=$expected \
    KILN_FLAKY_LISTED_PKG=$listed_pkg \
    KILN_FLAKY_LISTED_TEST=$listed_test \
    KILN_FLAKY_UNLISTED_TEST=$unlisted_test \
    NEXTEST_SHARDS=1 \
    KILN_NEXTEST_SERIAL=1 \
    PATH="$work:$PATH" \
    sh "$runner" >"$work/stdout" 2>"$work/stderr"
  rc=$?
  set -e
  if grep -q '^stub:' "$work/stderr"; then
    fail "$case_name: $(cat "$work/stderr")"
  fi
  if [ -f "$root/.config/nextest.toml" ] && grep -q 'slow-timeout' "$root/.config/nextest.toml"; then
    grep -q 'slow-timeout' "$work/seen-config" \
      || fail "$case_name: generated nextest config dropped slow-timeout"
  fi
  if cargo nextest --version >/dev/null 2>&1; then
    if ! cargo nextest show-config version --config-file "$work/seen-config" >/dev/null 2>"$work/show-config"; then
      cat "$work/show-config" >&2
      fail "$case_name: nextest rejected the generated config"
    fi
  fi
  workspace_calls=$(grep -F -c -e '--workspace' "$work/invocations" || true)
  server_calls=$(grep -F -c -e '-p kiln-server' "$work/invocations" || true)
  [ "$workspace_calls" -eq 1 ] || fail "$case_name: expected one workspace nextest run, got $workspace_calls"
  [ "$server_calls" -eq 1 ] || fail "$case_name: expected one kiln-server nextest run, got $server_calls"
  log=$root/target/flaky-retries.log
  [ -f "$log" ] || fail "$case_name: missing flaky retry log"
  cat "$work/stdout" "$work/stderr" >"$work/combined"
}

run_case retry-pass
[ "$rc" -eq 0 ] || fail "listed test passing on retry must be green (got $rc, stderr=$(cat "$work/stderr"))"
grep -F -q "flaky-retry: ${listed_pkg} ${listed_test} count=1" "$root/target/flaky-retries.log" \
  || fail 'retry pass must log the test name and count=1'
grep -F -q 'flaky-retry: total=1' "$root/target/flaky-retries.log" \
  || fail 'retry pass must count one retry'
grep -F -q "flaky-retry: ${listed_pkg} ${listed_test} count=1" "$work/combined" \
  || fail 'retry pass must print the retry summary'

run_case retry-fail
[ "$rc" -ne 0 ] || fail 'listed test that fails twice must be red'
grep -F -q "flaky-retry: ${listed_pkg} ${listed_test} count=1" "$root/target/flaky-retries.log" \
  || fail 'double failure must still log the one retry'
grep -F -q 'flaky-retry: total=1' "$root/target/flaky-retries.log" \
  || fail 'double failure must count one retry'

run_case unlisted-fail
[ "$rc" -ne 0 ] || fail 'unlisted test that fails once must be red'
grep -F -q "flaky-retry: total=0" "$root/target/flaky-retries.log" \
  || fail 'unlisted failure must not count a retry'
if grep -F -q "$unlisted_test" "$root/target/flaky-retries.log"; then
  fail 'unlisted failure must not be logged as a retry'
fi

printf 'flaky retry tests ok\n'
