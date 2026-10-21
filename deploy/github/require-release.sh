#!/bin/bash
# Stop before SSH when the tag has no published release.
set -euo pipefail
unset CDPATH

script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=/dev/null
source "$script_dir/guard.sh"
guard_github_event deploy

tag=${TAG:-latest}
repo=Jabbslad/kiln
case $tag in
  latest)
    url="https://api.github.com/repos/${repo}/releases/latest"
    ;;
  *)
    printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$' \
      || fail "invalid release tag: ${tag}"
    url="https://api.github.com/repos/${repo}/releases/tags/${tag}"
    ;;
esac

header=()
if [ -n "${GH_TOKEN:-}" ]; then
  header=(-H "Authorization: Bearer ${GH_TOKEN}")
fi
if ! payload=$(curl --silent --show-error --proto '=https' --proto-redir '=https' \
  --retry 3 --retry-delay 1 \
  --write-out '\n%{http_code}' \
  "${header[@]}" \
  "$url"); then
  fail "no published release for ${tag}"
fi
code=${payload##*$'\n'}
body=${payload%"$code"}
body=${body%$'\n'}
[ "$code" = 200 ] || fail "no published release for ${tag}"
printf '%s\n' "$body" | grep -Eq '"draft"[[:space:]]*:[[:space:]]*false' \
  || fail "no published release for ${tag}"
