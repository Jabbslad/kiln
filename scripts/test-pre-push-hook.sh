#!/bin/sh
# Prove pre-push runs check.sh, blocks on failure, and adds --release for deploy paths.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
hook=$root/.githooks/pre-push
scope_paths=$root/scripts/prepush-extra-check-paths.sh

fail() {
  printf 'test-pre-push-hook: %s\n' "$*" >&2
  exit 1
}

[ -x "$hook" ] || fail 'missing .githooks/pre-push'
[ -f "$scope_paths" ] || fail 'missing scripts/prepush-extra-check-paths.sh'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

setup_repo() {
  cd "$work"
  git init -b main >/dev/null 2>&1
  git config user.email test@test.test
  git config user.name test
  git config core.hooksPath .githooks
  mkdir -p scripts .githooks
  cp "$hook" .githooks/pre-push
  chmod +x .githooks/pre-push
  cp "$scope_paths" scripts/prepush-extra-check-paths.sh
  chmod +x scripts/prepush-extra-check-paths.sh
  cat >scripts/check.sh <<'EOF'
#!/bin/sh
set -eu
if [ $# -eq 0 ]; then
  line='scripts/check.sh'
else
  line="scripts/check.sh $*"
fi
printf '%s\n' "$line" >>"${CHECK_LOG:?}"
if [ -f "${CHECK_FAIL:-}" ]; then
  printf 'check stub failed\n' >&2
  exit 1
fi
exit 0
EOF
  chmod +x scripts/check.sh
  printf 'init\n' >README.md
  git add README.md scripts/check.sh .githooks/pre-push
  git commit -m init >/dev/null
}

run_hook() {
  local_ref=$1
  local_sha=$2
  remote_ref=$3
  remote_sha=$4
  : >"$CHECK_LOG"
  set +e
  export CHECK_LOG
  export CHECK_FAIL="${CHECK_FAIL:-}"
  printf '%s %s %s %s\n' "$local_ref" "$local_sha" "$remote_ref" "$remote_sha" \
    | sh .githooks/pre-push >"$work/stdout" 2>"$work/stderr"
  rc=$?
  set -e
  return "$rc"
}

setup_repo
zero=0000000000000000000000000000000000000000
head=$(git rev-parse HEAD)
CHECK_LOG=$work/log
export CHECK_LOG

touch "$work/fail"
CHECK_FAIL=$work/fail
export CHECK_FAIL
run_hook refs/heads/main "$head" refs/heads/main "$zero" || rc=$?
rc=${rc:-0}
[ "$rc" -ne 0 ] || fail 'failing check.sh must block pre-push'
unset rc
grep -q 'check stub failed' "$work/stderr" || fail 'pre-push must print failing check output'
grep -q 'scripts/check.sh' "$work/stderr" || fail 'pre-push must name the failing command'

rm -f "$work/fail"
unset CHECK_FAIL
export CHECK_FAIL=
run_hook refs/heads/main "$head" refs/heads/main "$zero" || rc=$?
rc=${rc:-0}
[ "$rc" -eq 0 ] || fail "passing check.sh must allow pre-push (got $rc, stderr=$(cat "$work/stderr"))"
unset rc
grep -qx 'scripts/check.sh' "$work/log" || fail 'passing push should run check.sh without extra args'

mkdir -p deploy
printf 'deploy\n' >deploy/stub.txt
git add deploy/stub.txt
git commit -m deploy >/dev/null
parent=$(git rev-parse HEAD~1)
head=$(git rev-parse HEAD)
run_hook refs/heads/main "$head" refs/heads/main "$parent" || rc=$?
rc=${rc:-0}
[ "$rc" -eq 0 ] || fail 'deploy push with passing check must succeed'
unset rc
grep -qx 'scripts/check.sh --release' "$work/log" || fail 'deploy diff must pass --release to check.sh'

mkdir -p docs
printf 'docs\n' >docs/only.md
git add docs/only.md
git commit -m docs >/dev/null
parent=$(git rev-parse HEAD~1)
head=$(git rev-parse HEAD)
run_hook refs/heads/main "$head" refs/heads/main "$parent" || rc=$?
rc=${rc:-0}
[ "$rc" -eq 0 ] || fail 'docs push must succeed'
unset rc
grep -qx 'scripts/check.sh --release' "$work/log" && fail 'docs-only diff must not pass --release'

# Agent bootstrap installs the hook, then a real local push is gated.
hooks_json=$root/.cursor/hooks.json
bootstrap=$root/.cursor/hooks/install-git-hooks.sh
[ -f "$hooks_json" ] || fail 'missing .cursor/hooks.json agent bootstrap'
[ -x "$bootstrap" ] || fail 'missing executable .cursor/hooks/install-git-hooks.sh'
grep -q 'beforeShellExecution' "$hooks_json" || fail 'agent bootstrap must run before shell commands'
grep -q 'install-git-hooks.sh' "$hooks_json" || fail 'agent bootstrap must install git hooks'
grep -q 'git push' "$hooks_json" || fail 'agent bootstrap must install hooks before git push'
for banned in "$hooks_json" "$bootstrap" "$root/scripts/install-hooks.sh" "$hook" "$root/README.md" "$root/AGENTS.md"; do
  if grep -q 'no-verify' "$banned"; then
    fail "do not document a hook bypass in $banned"
  fi
done

boot=$work/boot
mkdir -p "$boot/.cursor/hooks" "$boot/scripts" "$boot/.githooks"
cp "$bootstrap" "$boot/.cursor/hooks/install-git-hooks.sh"
cp "$root/scripts/install-hooks.sh" "$boot/scripts/install-hooks.sh"
cp "$hook" "$boot/.githooks/pre-push"
cp "$scope_paths" "$boot/scripts/prepush-extra-check-paths.sh"
chmod +x "$boot/.cursor/hooks/install-git-hooks.sh" "$boot/scripts/install-hooks.sh" \
  "$boot/.githooks/pre-push" "$boot/scripts/prepush-extra-check-paths.sh"
cat >"$boot/scripts/check.sh" <<'EOF'
#!/bin/sh
set -eu
if [ $# -eq 0 ]; then
  line='scripts/check.sh'
else
  line="scripts/check.sh $*"
fi
printf '%s\n' "$line" >>"${CHECK_LOG:?}"
if [ -f "${CHECK_FAIL:-}" ]; then
  printf 'check stub failed\n' >&2
  exit 1
fi
exit 0
EOF
chmod +x "$boot/scripts/check.sh"
(
  cd "$boot"
  git init -b main >/dev/null 2>&1
  git config user.email test@test.test
  git config user.name test
  printf 'init\n' >README.md
  git add README.md scripts/check.sh .githooks/pre-push scripts/prepush-extra-check-paths.sh
  git commit -m init >/dev/null
  printf '%s\n' '{"command":"git push origin HEAD:main","cwd":"."}' \
    | sh .cursor/hooks/install-git-hooks.sh >"$work/hook-out"
  grep -qx '{"permission":"allow"}' "$work/hook-out" \
    || fail "bootstrap must allow the push after install (out=$(cat "$work/hook-out"))"
  got=$(git config --get core.hooksPath)
  [ "$got" = ".githooks" ] || fail "bootstrap must set core.hooksPath=.githooks (got $got)"
  bare=$work/bare.git
  git init --bare "$bare" >/dev/null 2>&1
  git remote add origin "$bare"
  CHECK_LOG=$work/boot-log
  export CHECK_LOG
  touch "$work/boot-fail"
  CHECK_FAIL=$work/boot-fail
  export CHECK_FAIL
  set +e
  git push origin HEAD:main >"$work/push-out" 2>"$work/push-err"
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || fail 'failing check.sh must block git push'
  if git --git-dir="$bare" rev-parse --verify refs/heads/main >/dev/null 2>&1; then
    fail 'blocked push must not update the remote'
  fi
  rm -f "$work/boot-fail"
  unset CHECK_FAIL
  export CHECK_FAIL=
  set +e
  git push origin HEAD:main >"$work/push-out" 2>"$work/push-err"
  rc=$?
  set -e
  [ "$rc" -eq 0 ] || fail "passing check.sh must allow git push (got $rc, stderr=$(cat "$work/push-err"))"
  remote=$(git --git-dir="$bare" rev-parse refs/heads/main)
  [ "$remote" = "$(git rev-parse HEAD)" ] || fail 'passing push must update the remote'
)

printf 'pre-push hook tests ok\n'
