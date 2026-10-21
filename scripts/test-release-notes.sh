#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-release-notes: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git clone --quiet --bare "$root" "$work/bare"
git clone --quiet "$work/bare" "$work/repo"
cd "$work/repo"
git config user.email 'test@example.com'
git config user.name 'Test'

commit() {
  printf '%s\n' "$1" >"$2"
  git add "$2"
  git commit -qm "$1"
}

commit 'bootstrap' README.md
git tag v1.0.0
commit 'First feature (#1)' a.txt
commit 'Second feature (#2)' b.txt
git tag v1.0.1
commit 'Third feature (#3)' c.txt

json=$(KILN_RELEASE_NOTES_ROOT="$work/repo" sh "$root/scripts/release-notes-json.sh" v1.0.2 HEAD)
printf '%s\n' "$json" | grep -q '"version":"1.0.2"' || fail 'version missing'
printf '%s\n' "$json" | grep -q 'Third feature' || fail 'missing unreleased change'
printf '%s\n' "$json" | grep -qv 'Second feature' || fail 'unreleased range included prior release'
printf '%s\n' "$json" | grep -q '"pr":3' || fail 'missing pr number'

json=$(KILN_RELEASE_NOTES_ROOT="$work/repo" sh "$root/scripts/release-notes-json.sh" v1.0.1 v1.0.1)
printf '%s\n' "$json" | grep -q 'First feature' || fail 'tagged release missing first change'
printf '%s\n' "$json" | grep -q 'Second feature' || fail 'tagged release missing second change'
printf '%s\n' "$json" | grep -qv 'Third feature' || fail 'tagged release included future commit'

md=$(KILN_RELEASE_NOTES_ROOT="$work/repo" sh "$root/scripts/release-notes-markdown.sh" v1.0.1 v1.0.1)
printf '%s\n' "$md" | grep -q '^## v1.0.1' || fail 'markdown title missing'
printf '%s\n' "$md" | grep -q 'Second feature (#2)' || fail 'markdown bullet missing'

prev=$(printf 'v1.0.0\nv1.0.1\nv1.0.2\n' | sh "$root/scripts/previous-release-tag.sh" v1.0.2)
[ "$prev" = v1.0.1 ] || fail "previous tag is ${prev}"

printf 'test-release-notes ok\n'
