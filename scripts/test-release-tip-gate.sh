#!/bin/sh
# A build behind main skips the release. A build at the tip tags the next patch.
# Uses a temp repo and a stub gh. No external hosts.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-release-tip-gate: %s\n' "$*" >&2
  exit 1
}

if grep -n -E 'git push|git tag|--force' \
  "$root/scripts/release-tip-gate.sh" \
  "$root/scripts/plan-release.sh" >/dev/null; then
  fail 'release path moves tags or pushes git'
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

repo=$work/repo
git init -b main "$repo" >/dev/null
git -C "$repo" config user.email test@example.com
git -C "$repo" config user.name test
git -C "$repo" config commit.gpgsign false
printf 'old\n' >"$repo/file"
git -C "$repo" add file
git -C "$repo" commit -m old >/dev/null
old=$(git -C "$repo" rev-parse HEAD)
printf 'new\n' >"$repo/file"
git -C "$repo" add file
git -C "$repo" commit -m new >/dev/null
new=$(git -C "$repo" rev-parse HEAD)

stub=$work/bin
mkdir -p "$stub"
cat >"$stub/gh" <<'STUB'
#!/bin/sh
set -eu
log=${GH_STUB_LOG:?}
repo=${GH_STUB_REPO:?}
printf '%s\n' "$*" >>"$log"
cmd=$1
shift
case "$cmd" in
  api)
    method=GET
    path=
    call=$*
    while [ $# -gt 0 ]; do
      case "$1" in
        --method)
          method=$2
          shift 2
          ;;
        --jq)
          shift 2
          ;;
        -f)
          shift 2
          ;;
        --paginate)
          shift
          ;;
        repos/*)
          path=$1
          shift
          ;;
        *)
          shift
          ;;
      esac
    done
    case "$method:$path" in
      GET:*commits/main)
        git -C "$repo" rev-parse HEAD
        ;;
      POST:*git/refs)
        printf '%s\n' "$call" >>"${log}.post"
        ;;
      *)
        printf 'stub gh: unhandled %s %s\n' "$method" "$path" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    printf 'stub gh: unhandled %s\n' "$cmd" >&2
    exit 1
    ;;
esac
STUB
chmod 755 "$stub/gh"

run_gate() {
  sha=$1
  out=$2
  : >"$work/gh.log"
  rm -f "$work/gh.log.post"
  status=0
  RELEASE_SHA=$sha \
    GH_REPO=Jabbslad/kiln \
    GH_STUB_LOG=$work/gh.log \
    GH_STUB_REPO=$repo \
    PATH="$stub:$PATH" \
    sh "$root/scripts/release-tip-gate.sh" >"$out" || status=$?
  printf '%s\n' "$status"
}

behind_status=$(run_gate "$old" "$work/behind.out")
[ "$behind_status" -eq 2 ] || fail "behind gate exit $behind_status ($(cat "$work/behind.out"))"
grep -qx "release skipped: main moved to ${new}, the newer build releases" "$work/behind.out" \
  || fail "behind message: $(cat "$work/behind.out")"

tip_status=$(run_gate "$new" "$work/tip.out")
[ "$tip_status" -eq 0 ] || fail "tip gate exit $tip_status ($(cat "$work/tip.out"))"
if [ -s "$work/tip.out" ]; then
  fail "tip gate printed $(cat "$work/tip.out")"
fi

run_plan() {
  sha=$1
  out=$2
  github_out=$3
  : >"$work/gh.log"
  rm -f "$work/gh.log.post"
  status=0
  PLAN_RELEASE_PATHS=$root/scripts/fixtures/releasable/deploy.txt \
    PLAN_RELEASE_TAGS=$root/scripts/fixtures/releasable/tags.txt \
    PLAN_RELEASE_APPLY=1 \
    PLAN_RELEASE_SHA=$sha \
    GH_TOKEN=test-token \
    GH_REPO=Jabbslad/kiln \
    GH_STUB_LOG=$work/gh.log \
    GH_STUB_REPO=$repo \
    GITHUB_OUTPUT=$github_out \
    PATH="$stub:$PATH" \
    sh "$root/scripts/plan-release.sh" >"$out" 2>"$work/plan.err" || status=$?
  printf '%s\n' "$status"
}

behind_plan=$(run_plan "$old" "$work/behind-plan.out" "$work/behind-plan.github")
[ "$behind_plan" -eq 0 ] || fail "behind release failed ($behind_plan): $(cat "$work/plan.err") $(cat "$work/behind-plan.out")"
grep -qx "release skipped: main moved to ${new}, the newer build releases" "$work/behind-plan.out" \
  || fail "behind release message: $(cat "$work/behind-plan.out")"
grep -qx 'release=false' "$work/behind-plan.github" || fail 'behind release did not set release=false'
if [ -f "$work/gh.log.post" ]; then
  fail "behind release created a tag: $(cat "$work/gh.log.post")"
fi

tip_plan=$(run_plan "$new" "$work/tip-plan.out" "$work/tip-plan.github")
[ "$tip_plan" -eq 0 ] || fail "tip release failed ($tip_plan): $(cat "$work/plan.err") $(cat "$work/tip-plan.out")"
grep -qx 'release=true' "$work/tip-plan.github" || fail "tip release output: $(cat "$work/tip-plan.github")"
grep -qx 'tag=v0.4.20' "$work/tip-plan.github" || fail "tip tag: $(cat "$work/tip-plan.github")"
grep -q 'release skipped' "$work/tip-plan.out" && fail 'tip release was skipped'
[ -f "$work/gh.log.post" ] || fail 'tip release did not create a tag'
grep -q 'refs/tags/v0.4.20' "$work/gh.log.post" || fail "tip post: $(cat "$work/gh.log.post")"
grep -q "$new" "$work/gh.log" || fail 'tip release did not target the main commit'

build=$root/.github/workflows/build.yml
deploy=$root/.github/workflows/deploy.yml
grep -q 'scripts/release-tip-gate.sh' "$root/scripts/plan-release.sh" \
  || fail 'plan-release does not gate the tag'
grep -q 'scripts/release-tip-gate.sh' "$build" \
  || fail 'build.yml does not gate publish'
# shellcheck disable=SC2016
grep -F -q 'sha: ${{ github.sha }}' "$build" \
  || fail 'build does not pass the commit to deploy'
grep -q 'scripts/release-tip-gate.sh' "$deploy" \
  || fail 'deploy.yml does not gate deploy'
grep -q "needs.draft-release.outputs.published == 'true'" "$build" \
  || fail 'deploy job does not require a publish'
grep -q "inputs.sha == ''" "$deploy" \
  || fail 'manual deploy must run without the tip check'
grep -q 'release-tip-gate.sh' "$root/scripts/batch-merge.sh" \
  || fail 'batch-merge does not use the tip gate'

# Build A skips because main moved to B. B's build then fails.
# A green B would tag. A failed B never reaches tag, publish, or deploy.
# A's skip log names B's full commit. Neither build tags or deploys.
finish_build() {
  sha=$1
  label=$2
  if [ "$label" = B ]; then
    printf 'build B failed\n' >&2
    return 1
  fi
  plan_status=$(run_plan "$sha" "$work/${label}.out" "$work/${label}.github")
  [ "$plan_status" -eq 0 ] || return "$plan_status"
  gate_status=0
  RELEASE_SHA=$sha \
    GH_REPO=Jabbslad/kiln \
    GH_STUB_LOG=$work/gh.log \
    GH_STUB_REPO=$repo \
    PATH="$stub:$PATH" \
    sh "$root/scripts/release-tip-gate.sh" >"$work/${label}-deploy.out" || gate_status=$?
  if [ "$gate_status" -eq 0 ]; then
    printf 'deploy %s\n' "$sha" >>"$work/deploy.log"
    return 0
  fi
  if [ "$gate_status" -eq 2 ]; then
    return 0
  fi
  return "$gate_status"
}

if ! finish_build "$old" A >"$work/a-finish.out" 2>"$work/a-finish.err"; then
  fail "build A should stay green ($(cat "$work/a-finish.err"))"
fi
printf '%s\n' "$new" | grep -Eq '^[0-9a-f]{40}$' || fail 'B is not a full sha'
named=$(sed -n 's/^release skipped: main moved to \([0-9a-f]\{40\}\), the newer build releases$/\1/p' "$work/A.out")
[ "$named" = "$new" ] || fail "build A skip log named ${named:-nothing}, want ${new}"
[ "$named" != "$old" ] || fail 'build A skip log named itself'
grep -qx 'release=false' "$work/A.github" || fail 'build A still planned a release'
deploy_named=$(sed -n 's/^release skipped: main moved to \([0-9a-f]\{40\}\), the newer build releases$/\1/p' "$work/A-deploy.out")
[ "$deploy_named" = "$new" ] || fail "build A deploy skip log named ${deploy_named:-nothing}, want ${new}"
if [ -f "$work/deploy.log" ]; then
  fail "build A deployed: $(cat "$work/deploy.log")"
fi

if finish_build "$new" B >"$work/b-finish.out" 2>"$work/b-finish.err"; then
  fail 'build B should fail'
fi
grep -qx 'build B failed' "$work/b-finish.err" || fail "build B did not fail closed: $(cat "$work/b-finish.err")"
if [ -f "$work/B.out" ] || [ -f "$work/B.github" ] || [ -f "$work/B-deploy.out" ]; then
  fail 'build B reached tag or deploy after failing'
fi
if [ -f "$work/gh.log.post" ]; then
  fail "a tag was created after A skipped and B failed: $(cat "$work/gh.log.post")"
fi
if grep -q 'refs/tags/' "$work/gh.log" 2>/dev/null; then
  fail "a tag API call was recorded: $(cat "$work/gh.log")"
fi
if [ -f "$work/deploy.log" ]; then
  fail "a deploy ran after A skipped and B failed: $(cat "$work/deploy.log")"
fi

printf 'release tip gate passed\n'
