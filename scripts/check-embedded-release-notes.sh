#!/bin/sh
# Verify the built dashboard bundle embeds release notes for TAG.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'check-embedded-release-notes: %s\n' "$*" >&2
  exit 1
}

tag=${1:-${VERSION:-}}
[ -n "$tag" ] || fail 'usage: check-embedded-release-notes.sh TAG'

printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "tag ${tag} is not stable"

export VERSION="$tag"
sh "$root/scripts/write-release-notes.sh"
expected=$root/web/src/release-notes.json

(
  cd "$root/web"
  npm run build >/dev/null
)

EXPECTED="$expected" ASSETS="$root/web/dist/assets" node <<'NODE'
const fs = require('node:fs')
const path = require('node:path')
const expected = JSON.parse(fs.readFileSync(process.env.EXPECTED, 'utf8'))
const assets = process.env.ASSETS
const bundles = fs
  .readdirSync(assets)
  .filter((name) => name.endsWith('.js'))
  .map((name) => fs.readFileSync(path.join(assets, name), 'utf8'))
if (bundles.length === 0) {
  console.error('dashboard bundle js missing')
  process.exit(1)
}
const versionNeedles = [
  'version:"' + expected.version + '"',
  '"version":"' + expected.version + '"',
  'version:`' + expected.version + '`',
]
const source = bundles.find((chunk) => versionNeedles.some((needle) => chunk.includes(needle)))
if (!source) {
  console.error('embedded version missing from bundle')
  process.exit(1)
}
if (!source.includes(expected.title)) {
  console.error('embedded title missing from bundle')
  process.exit(1)
}
for (const change of expected.changes.slice(0, 3)) {
  if (!source.includes(change.summary)) {
    console.error('embedded change missing: ' + change.summary)
    process.exit(1)
  }
}
NODE

printf 'embedded release notes ok %s\n' "$tag"
