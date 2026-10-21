#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-plan-release: %s\n' "$*" >&2
  exit 1
}

if grep -n -E 'git push|git tag|--force' "$root/scripts/plan-release.sh" >/dev/null; then
  fail 'plan-release moves tags or pushes git'
fi
grep -q 'tag already exists' "$root/scripts/plan-release.sh" || fail 'plan-release does not fail closed'
grep -q -- '--check' "$root/scripts/plan-release.sh" || fail 'plan-release does not check the tag'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
sha=0123456789abcdef0123456789abcdef01234567

PLAN_RELEASE_PATHS=$root/scripts/fixtures/releasable/docs-only.txt \
  PLAN_RELEASE_TAGS=$root/scripts/fixtures/releasable/tags.txt \
  PLAN_RELEASE_APPLY=0 \
  PLAN_RELEASE_SHA=$sha \
  GITHUB_OUTPUT=$work/docs.out \
  sh "$root/scripts/plan-release.sh" >"$work/docs.txt"
grep -q '^release=false$' "$work/docs.out" || fail 'docs-only planned a release'
grep -q '^tag=$' "$work/docs.out" || fail 'docs-only named a tag'
if grep -q 'dry-run tag' "$work/docs.txt"; then
  fail 'docs-only printed a tag'
fi

PLAN_RELEASE_PATHS=$root/scripts/fixtures/releasable/deploy.txt \
  PLAN_RELEASE_TAGS=$root/scripts/fixtures/releasable/tags.txt \
  PLAN_RELEASE_APPLY=0 \
  PLAN_RELEASE_SHA=$sha \
  GITHUB_OUTPUT=$work/code.out \
  sh "$root/scripts/plan-release.sh" >"$work/code.txt"
grep -q '^release=true$' "$work/code.out" || fail 'deploy diff was not releasable'
grep -q '^tag=v0.4.20$' "$work/code.out" || fail "tag output: $(cat "$work/code.out")"
grep -q '^from=v0.4.17$' "$work/code.out" || fail "from output: $(cat "$work/code.out")"
grep -q "dry-run tag v0.4.20 sha ${sha}" "$work/code.txt" || fail 'missing dry-run tag line'
grep -q 'dry-run release v0.4.20' "$work/code.txt" || fail 'missing dry-run release line'
printf 'plan release passed\n'
