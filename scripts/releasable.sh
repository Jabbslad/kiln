#!/bin/sh
# Read changed paths on stdin. Print release=true unless every path is docs.
# An empty diff is not a release. deploy/, scripts/, and .github/ are releases.
set -eu

is_doc() {
  rel=$1
  case $rel in
    '' | */) return 1 ;;
    deploy/* | scripts/* | .github/*) return 1 ;;
  esac
  name=${rel##*/}
  case $name in
    Cargo.toml | Cargo.lock | package-lock.json | npm-shrinkwrap.json | yarn.lock | pnpm-lock.yaml | bun.lock | bun.lockb | poetry.lock | uv.lock | composer.lock | Gemfile.lock | go.sum | *.lock)
      return 1
      ;;
    *.md) return 0 ;;
  esac
  case $rel in
    docs/*)
      case $name in
        *.rs | *.py | *.sh | *.bash | *.js | *.mjs | *.cjs | *.ts | *.tsx | *.jsx | *.toml | *.yml | *.yaml | *.json)
          return 1
          ;;
        *) return 0 ;;
      esac
      ;;
  esac
  return 1
}

release=false
seen=0
while IFS= read -r line || [ -n "$line" ]; do
  rel=$(printf '%s' "$line" | awk '{ gsub("\\\\", "/"); print }')
  rel=$(printf '%s' "$rel" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
  rel=${rel#/}
  case $rel in
    ./*) rel=${rel#./} ;;
  esac
  [ -n "$rel" ] || continue
  seen=1
  if ! is_doc "$rel"; then
    release=true
  fi
done

if [ "$seen" -eq 0 ]; then
  printf 'release=false\n'
else
  printf 'release=%s\n' "$release"
fi
