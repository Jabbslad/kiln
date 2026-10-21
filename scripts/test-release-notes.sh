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

git tag -l 'v*' | while read -r tag_name; do
  git tag -d "$tag_name"
done

json=$(KILN_RELEASE_NOTES_ROOT="$work/repo" KILN_FETCH_RELEASE_TAGS=0 KILN_RELEASE_NOTES_STRICT=0 \
  sh "$root/scripts/release-notes-json.sh" v1.0.2 HEAD)
printf '%s\n' "$json" | grep -q '"changes":\[\]' || fail 'PR-like build must fall back to empty notes'
printf '%s\n' "$json" | grep -q '"version":"1.0.2"' || fail 'PR-like fallback missing version'

if KILN_RELEASE_NOTES_ROOT="$work/repo" KILN_FETCH_RELEASE_TAGS=0 KILN_RELEASE_NOTES_STRICT=1 \
  sh "$root/scripts/release-notes-json.sh" v1.0.2 HEAD >"$work/strict.out" 2>"$work/strict.err"; then
  fail 'tag build must fail without tag history'
fi
grep -q 'release-notes-json: v1.0.2: no previous stable tag in history' "$work/strict.err" \
  || fail "strict failure did not name the tag: $(cat "$work/strict.err")"

git tag v1.0.0
git tag v1.0.1
commit 'Only merge commit' d.txt
if KILN_RELEASE_NOTES_ROOT="$work/repo" KILN_FETCH_RELEASE_TAGS=0 KILN_RELEASE_NOTES_STRICT=1 \
  sh "$root/scripts/release-notes-json.sh" v1.0.2 v1.0.1 >"$work/empty-range.out" 2>"$work/empty-range.err"; then
  fail 'tag build must fail when the release range has no commits'
fi
grep -q 'release-notes-json: v1.0.2: no release notes between v1.0.1 and v1.0.1' "$work/empty-range.err" \
  || fail "empty-range failure missing tag: $(cat "$work/empty-range.err")"

# Unpublished intermediate tag (e.g. v0.4.46): published-base walks back to last GitHub release.
git clone --quiet "$work/bare" "$work/pub"
cd "$work/pub"
git config user.email 'test@example.com'
git config user.name 'Test'
commit 'bootstrap' README.md
git tag v1.0.0
commit 'Before gap (#10)' a.txt
git tag v1.0.1
commit 'In unpublished gap (#11)' b.txt
git tag v1.0.2
commit 'After gap (#12)' c.txt
git tag v1.0.3
printf 'v1.0.0\nv1.0.1\nv1.0.3\n' >"$work/published-tags.txt"
prev_pub=$(KILN_RELEASE_NOTES_ROOT="$work/pub" KILN_RELEASE_NOTES_PUBLISHED_BASE=1 \
  KILN_PUBLISHED_RELEASE_TAGS="$work/published-tags.txt" \
  sh "$root/scripts/previous-release-notes-from.sh" v1.0.3)
[ "$prev_pub" = v1.0.1 ] || fail "published-base previous tag is ${prev_pub}, want v1.0.1"
prev_git=$(KILN_RELEASE_NOTES_ROOT="$work/pub" sh "$root/scripts/previous-release-notes-from.sh" v1.0.3)
[ "$prev_git" = v1.0.2 ] || fail "git-only previous tag is ${prev_git}, want v1.0.2"
json=$(KILN_RELEASE_NOTES_ROOT="$work/pub" KILN_RELEASE_NOTES_PUBLISHED_BASE=1 \
  KILN_PUBLISHED_RELEASE_TAGS="$work/published-tags.txt" \
  sh "$root/scripts/release-notes-json.sh" v1.0.3 v1.0.3)
printf '%s\n' "$json" | grep -q 'In unpublished gap' || fail 'published base missing commit on unpublished tag'
printf '%s\n' "$json" | grep -q 'After gap' || fail 'published base missing commit after unpublished tag'
printf '%s\n' "$json" | grep -qv 'Before gap' || fail 'published base must not repeat prior published release'

printf 'test-release-notes ok\n'
