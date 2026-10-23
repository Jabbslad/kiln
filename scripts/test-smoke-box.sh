#!/bin/bash
# Hermetic deploy smoke: unique box name, stale kiln-smoke* cleanup, delete on failure.
set -euo pipefail

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-smoke-box: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
bin=$work/bin
mkdir -p "$bin"

cat >"$bin/kiln" <<'EOF'
#!/bin/bash
set -euo pipefail
state=${KILN_STUB_STATE:?}
log=${KILN_STUB_LOG:?}
printf '%s\n' "$*" >>"$log"
cmd=
name=
prev=
for arg in "$@"; do
  case "$arg" in
    add | list | create | exec | delete) cmd=$arg ;;
  esac
  if [ "$prev" = --name ]; then
    name=$arg
  fi
  prev=$arg
done
case "$cmd" in
  add) exit 0 ;;
  list)
    if [ ! -s "$state" ]; then
      printf 'No boxes yet. Create one with `kiln create`.\n'
      exit 0
    fi
    printf 'NAME  ID  STATE  MEMORY  VCPUS\n'
    while IFS=' ' read -r bname bid bstate; do
      [ -n "$bname" ] || continue
      printf '%s  %s  %s  4096 MiB  1\n' "$bname" "$bid" "$bstate"
    done <"$state"
    exit 0
    ;;
  create)
    [ -n "$name" ] || exit 2
    if [ -s "$state" ] && awk -v name="$name" '$1 == name { found = 1 } END { exit !found }' "$state"; then
      printf "box name '%s' is already in use\n" "$name" >&2
      exit 1
    fi
    count=$(wc -l <"$state" | tr -d '[:space:]')
    id=$(printf '%032x' "$((16 + count))")
    printf '%s %s running\n' "$name" "$id" >>"$state"
    printf '%s\n' "{\"id\":\"op\",\"box_id\":\"${id}\",\"state\":\"succeeded\",\"result\":{\"type\":\"box\",\"id\":\"${id}\",\"name\":\"${name}\",\"state\":\"running\",\"memory_mib\":4096,\"vcpus\":1},\"error\":null}"
    exit 0
    ;;
  exec)
    if [ "${KILN_STUB_FAIL:-}" = exec ]; then
      exit 1
    fi
    printf 'kiln-smoke-ok'
    exit 0
    ;;
  delete)
    target=
    for arg in "$@"; do
      target=$arg
    done
    [ -n "$target" ] || exit 2
    tmp=$(mktemp)
    found=0
    if [ -s "$state" ]; then
      while IFS=' ' read -r bname bid bstate; do
        [ -n "$bname" ] || continue
        if [ "$bname" = "$target" ] || [ "$bid" = "$target" ]; then
          found=1
          continue
        fi
        printf '%s %s %s\n' "$bname" "$bid" "$bstate"
      done <"$state" >"$tmp"
    fi
    mv "$tmp" "$state"
    [ "$found" = 1 ]
    exit 0
    ;;
  *)
    printf 'kiln stub: unknown command\n' >&2
    exit 99
    ;;
esac
EOF

cat >"$bin/kiln-api" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"${KILN_STUB_LOG:?}"
case "$*" in
  *self-check*)
    printf 'smoke: launch timing checked\n'
    exit 0
    ;;
  *) exit 1 ;;
esac
EOF
chmod 755 "$bin/kiln" "$bin/kiln-api"

resolved=$(PATH="$bin:/usr/bin:/bin" command -v kiln)
[ "$resolved" = "$bin/kiln" ] || fail "kiln stub is not on PATH: ${resolved}"

seed_stale() {
  state=$1
  cat >"$state" <<'EOF'
kiln-smoke aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa stopped
kiln-smoke-earlier bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb stopped
my-kiln-smoke cccccccccccccccccccccccccccccccc running
customer-web dddddddddddddddddddddddddddddddd running
EOF
}

seed_empty() {
  : >"$1"
}

run_case() {
  mode=$1
  run_id=$2
  attempt=$3
  seed=$4
  case_dir=$work/$mode-$attempt
  mkdir -p "$case_dir"
  state=$case_dir/boxes
  log=$case_dir/kiln.log
  out=$case_dir/stdout
  err=$case_dir/stderr
  token=$case_dir/admin.token
  ca=$case_dir/ca.crt
  : >"$log"
  printf '%s\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' >"$token"
  printf '%s\n' 'fixture-ca' >"$ca"
  if [ "$seed" = stale ]; then
    seed_stale "$state"
  else
    seed_empty "$state"
  fi
  stub_fail=
  if [ "$mode" = fail ]; then
    stub_fail='exec'
  fi
  set +e
  env \
    PATH="$bin:/usr/bin:/bin" \
    KILN_BIN="$bin/kiln" \
    KILN_API_BIN="$bin/kiln-api" \
    KILN_ADMIN_TOKEN_FILE="$token" \
    KILN_CA_FILE="$ca" \
    KILN_STUB_STATE="$state" \
    KILN_STUB_LOG="$log" \
    KILN_STUB_FAIL="$stub_fail" \
    GITHUB_RUN_ID="$run_id" \
    GITHUB_RUN_ATTEMPT="$attempt" \
    bash "$root/deploy/github/smoke.sh" 127.0.0.1 >"$out" 2>"$err"
  rc=$?
  set -e
  box_name="kiln-smoke-${run_id}-${attempt}"
  if [ "$mode" = fail ]; then
    [ "$rc" -ne 0 ] || fail "failing smoke exited 0: $(cat "$err")"
  else
    [ "$rc" -eq 0 ] || fail "passing smoke exited ${rc}: $(cat "$err")"
  fi
  grep -F -- "--name ${box_name}" "$log" >/dev/null \
    || fail "smoke did not create ${box_name}: $(cat "$log")"
  if grep -E -- '--name kiln-smoke($|[[:space:]])' "$log" >/dev/null; then
    fail "smoke still creates the fixed name kiln-smoke: $(cat "$log")"
  fi
  grep -F -- "--require-launch-timing ${box_name}" "$log" >/dev/null \
    || fail "launch timing check did not use ${box_name}: $(cat "$log")"
  if [ -s "$state" ] && awk -v prefix='kiln-smoke' '$1 ~ "^" prefix { found = 1 } END { exit !found }' "$state"; then
    fail "smoke box still present after ${mode}: $(cat "$state")"
  fi
  if [ "$seed" = stale ]; then
    kept=$(awk '{ print $1 }' "$state" | sort)
    want=$(printf '%s\n' customer-web my-kiln-smoke | sort)
    [ "$kept" = "$want" ] || fail "stale cleanup touched unrelated boxes: [${kept}]"
    grep -E 'delete kiln-smoke($|[[:space:]])' "$log" >/dev/null \
      || fail "legacy kiln-smoke box was not deleted: $(cat "$log")"
    if grep -E 'delete (customer-web|my-kiln-smoke)($|[[:space:]])' "$log" >/dev/null; then
      fail "deleted a box outside the kiln-smoke prefix: $(cat "$log")"
    fi
  fi
}

run_case fail 39877921457 2 stale
run_case pass 39877921457 1 stale
run_case pass 39877921457 3 empty

printf 'smoke box tests ok\n'
