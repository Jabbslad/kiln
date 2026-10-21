#!/bin/sh
# Pull requests that change dashboard or CLI output must include proof in the PR body
# (screenshot or video link), or a line: proof: n/a
set -eu

fail() {
  printf 'pr-proof-guard: %s\n' "$*" >&2
  exit 1
}

guarded=0

path_guarded() {
  case "$1" in
    web/src/*|web/public/*|web/index.html) return 0 ;;
    crates/kiln-client/src/*) return 0 ;;
    crates/kiln-runtime/src/bin/*) return 0 ;;
    *) return 1 ;;
  esac
}

while IFS= read -r path || [ -n "$path" ]; do
  [ -n "$path" ] || continue
  if path_guarded "$path"; then
    guarded=1
  fi
done

if [ "$guarded" -eq 0 ]; then
  exit 0
fi

body=${PR_BODY:-}

if printf '%s\n' "$body" | grep -Fxq 'proof: n/a'; then
  exit 0
fi

if printf '%s\n' "$body" | grep -Eiq \
  '!\[[^]]*\]\([^)]+\.(png|webp|gif|jpe?g|webm|mp4)(\?[^)]*)?\)'; then
  exit 0
fi

if printf '%s\n' "$body" | grep -Eiq \
  '\]\([^)]+\.(png|webp|gif|jpe?g|webm|mp4)(\?[^)]*)?\)'; then
  exit 0
fi

if printf '%s\n' "$body" | grep -Eiq '<(img|video)[^>]+src="[^"]+\.(png|webp|gif|jpe?g|webm|mp4)'; then
  exit 0
fi

fail 'guarded paths changed without PR proof (add ## Proof with a screenshot or video link, or a body line: proof: n/a)'
