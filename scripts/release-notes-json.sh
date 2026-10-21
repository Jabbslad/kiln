#!/bin/sh
# Build release notes JSON for TAG from git history since the previous stable tag.
# Optional second argument is the end ref (defaults to TAG when it resolves, else HEAD).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
repo_root=${KILN_RELEASE_NOTES_ROOT:-$root}
cd "$repo_root"

fail() {
  printf 'release-notes-json: %s\n' "$*" >&2
  exit 1
}

tag=${1:-}
[ -n "$tag" ] || fail 'usage: release-notes-json.sh TAG [END_REF]'

printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "tag ${tag} is not stable"

end=${2:-}
if [ -z "$end" ]; then
  if git rev-parse --verify --quiet "${tag}^{commit}" >/dev/null; then
    end=$tag
  else
    end=HEAD
  fi
fi

repo=${KILN_RELEASE_NOTES_REPO:-Jabbslad/kiln}
version=${tag#v}
if [ "${KILN_FETCH_RELEASE_TAGS:-1}" = 1 ]; then
  git fetch --tags origin >/dev/null 2>&1 || true
fi
from=
if from=$(git tag -l 'v*' | sh "$root/scripts/previous-release-tag.sh" "$tag" 2>/dev/null); then
  :
else
  from=
fi

if [ -z "$from" ]; then
  printf '{"version":"%s","title":"%s","changes":[],"releaseUrl":"https://github.com/%s/releases/tag/%s"}\n' \
    "$version" "$tag" "$repo" "$tag"
  exit 0
fi

git log "${from}..${end}" --first-parent --pretty=format:%s | awk -v version="$version" -v tag="$tag" -v repo="$repo" '
function json_escape(text,    out, i, ch, code) {
  out = ""
  for (i = 1; i <= length(text); i++) {
    ch = substr(text, i, 1)
    if (ch == "\\") out = out "\\\\"
    else if (ch == "\"") out = out "\\\""
    else if (ch == "\t") out = out "\\t"
    else if (ch == "\r") out = out "\\r"
    else if (ch == "\n") out = out "\\n"
    else out = out ch
  }
  return out
}
BEGIN {
  printf "{"
  printf "\"version\":\"%s\",", version
  printf "\"title\":\"%s\",", tag
  printf "\"changes\":["
  first = 1
}
{
  subject = $0
  if (subject == "") next
  pr = ""
  if (match(subject, / \(#[0-9]+\)$/)) {
    pr = substr(subject, RSTART + 3, RLENGTH - 4)
    summary = substr(subject, 1, RSTART - 1)
    key = "pr:" pr
  } else {
    summary = subject
    key = "subject:" subject
  }
  if (key in seen) next
  seen[key] = 1
  if (!first) printf ","
  first = 0
  if (pr != "") {
    printf "{\"summary\":\"%s\",\"pr\":%s}", json_escape(summary), pr
  } else {
    printf "{\"summary\":\"%s\"}", json_escape(summary)
  }
}
END {
  printf "],"
  printf "\"releaseUrl\":\"https://github.com/%s/releases/tag/%s\"}", repo, tag
  printf "\n"
}
'
