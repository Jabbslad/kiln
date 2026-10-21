#!/bin/sh
# Print the highest stable vX.Y.Z tag strictly before TAG.
# Tag lines on stdin may be plain tags or ls-remote rows.
set -eu

fail() {
  printf 'previous-release-tag: %s\n' "$*" >&2
  exit 1
}

tag=${1:-}
[ -n "$tag" ] || fail 'usage: previous-release-tag.sh TAG'

printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "tag ${tag} is not stable"

target=${tag#v}
t_major=${target%%.*}
t_tail=${target#*.}
t_minor=${t_tail%%.*}
t_patch=${t_tail#*.}

best_major=-1
best_minor=-1
best_patch=-1
found=0
while IFS= read -r line || [ -n "$line" ]; do
  candidate=$(printf '%s\n' "$line" | awk -F '\t' '{ print $NF }' | sed -e 's#^refs/tags/##' -e 's/\^{}$//')
  [ "$candidate" = "$tag" ] && continue
  printf '%s\n' "$candidate" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' || continue
  rest=${candidate#v}
  major=${rest%%.*}
  tail=${rest#*.}
  minor=${tail%%.*}
  patch=${tail#*.}
  if [ "$major" -gt "$t_major" ] \
    || { [ "$major" -eq "$t_major" ] && [ "$minor" -gt "$t_minor" ]; } \
    || { [ "$major" -eq "$t_major" ] && [ "$minor" -eq "$t_minor" ] && [ "$patch" -ge "$t_patch" ]; }; then
    continue
  fi
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

[ "$found" -eq 1 ] || fail "no previous tag before ${tag}"
printf 'v%s.%s.%s\n' "$best_major" "$best_minor" "$best_patch"
