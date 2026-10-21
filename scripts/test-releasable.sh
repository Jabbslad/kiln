#!/bin/sh
# Planted path lists. Docs-only is not a release. Code, workflow, deploy, and mixed are.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-releasable: %s\n' "$*" >&2
  exit 1
}

classify() {
  sh "$root/scripts/releasable.sh" < "$1"
}

[ "$(classify "$root/scripts/fixtures/releasable/docs-only.txt")" = release=false ] \
  || fail 'docs-only diff released'
[ "$(classify "$root/scripts/fixtures/releasable/code.txt")" = release=true ] \
  || fail 'code diff did not release'
[ "$(classify "$root/scripts/fixtures/releasable/workflow.txt")" = release=true ] \
  || fail 'workflow diff did not release'
[ "$(classify "$root/scripts/fixtures/releasable/deploy.txt")" = release=true ] \
  || fail 'deploy diff did not release'
[ "$(classify "$root/scripts/fixtures/releasable/mixed.txt")" = release=true ] \
  || fail 'mixed diff did not release'
[ "$(printf '\n' | sh "$root/scripts/releasable.sh")" = release=false ] \
  || fail 'empty diff released'
printf 'releasable fixtures passed\n'
