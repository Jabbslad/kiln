#!/bin/sh
# Render GitHub release notes markdown for TAG from the same git range as release-notes-json.sh.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
repo_root=${KILN_RELEASE_NOTES_ROOT:-$root}
cd "$repo_root"
. "$root/scripts/release-notes-mode.sh"

fail() {
  printf 'release-notes-markdown: %s\n' "$*" >&2
  exit 1
}

strict_fail() {
  fail "$1: $2"
}

tag=${1:-}
[ -n "$tag" ] || fail 'usage: release-notes-markdown.sh TAG [END_REF]'

end=${2:-}
if [ -z "$end" ]; then
  if git rev-parse --verify --quiet "${tag}^{commit}" >/dev/null; then
    end=$tag
  else
    end=HEAD
  fi
fi

if [ "${KILN_FETCH_RELEASE_TAGS:-1}" = 1 ]; then
  git fetch --tags origin >/dev/null 2>&1 || true
fi
from=
if from=$(sh "$root/scripts/previous-release-notes-from.sh" "$tag" 2>/dev/null); then
  :
else
  from=
fi
repo=${KILN_RELEASE_NOTES_REPO:-Jabbslad/kiln}

if [ -z "$from" ]; then
  if release_notes_strict_enabled; then
    strict_fail "$tag" 'no previous stable tag in history'
  fi
  printf '## %s\n\n' "$tag"
  printf '### Changes\n\n'
  printf '_No release notes were generated._\n\n'
  printf 'Full release: https://github.com/%s/releases/tag/%s\n' "$repo" "$tag"
  exit 0
fi

if ! git log "${from}..${end}" --first-parent --pretty=format:%s | grep -q .; then
  if release_notes_strict_enabled; then
    strict_fail "$tag" "no release notes between ${from} and ${end}"
  fi
  printf '## %s\n\n' "$tag"
  printf '### Changes\n\n'
  printf '_No release notes were generated._\n\n'
  printf 'Full release: https://github.com/%s/releases/tag/%s\n' "$repo" "$tag"
  exit 0
fi

printf '## %s\n\n' "$tag"
printf '### Changes\n\n'
git log "${from}..${end}" --first-parent --pretty=format:%s | awk '
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
  if (pr != "") {
    printf "* %s (#%s)\n", summary, pr
  } else {
    printf "* %s\n", summary
  }
}
'
printf '\nFull release: https://github.com/%s/releases/tag/%s\n' "$repo" "$tag"
