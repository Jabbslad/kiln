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
if [ -n "${CHECK_NESTED_GIT:-}" ]; then
  git init --bare "$CHECK_NESTED_GIT" >/dev/null 2>&1
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
  export CHECK_FAIL=${CHECK_FAIL:-}
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

# Git exports repository selectors to hooks. A check creating its own fixture
# must not turn the repository being pushed into that fixture's bare Git dir.
CHECK_NESTED_GIT=$work/disposable.git
export CHECK_NESTED_GIT
GIT_DIR=$work/.git GIT_WORK_TREE=$work run_hook refs/heads/main "$head" refs/heads/main "$parent" \
  || fail 'hook failed with Git-supplied repository environment'
[ "$(git config core.bare)" = false ] || fail 'nested test changed the real repository configuration'
[ "$(git rev-parse HEAD)" = "$head" ] || fail 'nested test changed the branch being pushed'
[ -d "$CHECK_NESTED_GIT/objects" ] || fail 'nested test did not use its disposable repository'

printf 'pre-push hook tests ok\n'
