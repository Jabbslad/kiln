#!/bin/sh
# Next stable patch after the highest vX.Y.Z tag on stdin.
# Lines may be "v1.2.3", "refs/tags/v1.2.3", or "sha<TAB>refs/tags/v1.2.3".
# --check TAG fails when TAG is already in the list.
set -eu

fail() {
  printf 'next-release-tag: %s\n' "$*" >&2
  exit 1
}

check=
if [ "${1:-}" = --check ]; then
  check=${2:-}
  [ -n "$check" ] || fail 'usage: next-release-tag.sh --check TAG'
  shift 2
fi

best_major=-1
best_minor=-1
best_patch=-1
found=0
present=0
while IFS= read -r line || [ -n "$line" ]; do
  tag=$(printf '%s\n' "$line" | awk -F '\t' '{ print $NF }' | sed -e 's#^refs/tags/##' -e 's/\^{}$//')
  if [ -n "$check" ] && [ "$tag" = "$check" ]; then
    present=1
  fi
  printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' || continue
  rest=${tag#v}
  major=${rest%%.*}
  tail=${rest#*.}
  minor=${tail%%.*}
  patch=${tail#*.}
  if [ "$found" -eq 0 ] \
    || [ "$major" -gt "$best_major" ] \
    || { [ "$major" -eq "$best_major" ] && [ "$minor" -gt "$best_minor" ]; } \
    || { [ "$major" -eq "$best_major" ] && [ "$minor" -eq "$best_minor" ] && [ "$patch" -gt "$best_patch" ]; }; then
    best_major=$major
    best_minor=$minor
    best_patch=$patch
    found=1
  fi
done

if [ -n "$check" ]; then
  [ "$present" -eq 0 ] || fail "refusing ${check}; tag already exists"
  exit 0
fi

[ "$found" -eq 1 ] || fail 'no stable tags'
printf 'v%s.%s.%s\n' "$best_major" "$best_minor" "$((best_patch + 1))"
