#!/bin/sh
# Capture before/after dashboard screenshots for release-notes proof in PRs.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root/web"

out=${1:-$root/docs/artifacts/dashboard-release-notes-proof}
case $out in
  /*) ;;
  *) out=$root/$out ;;
esac
mkdir -p "$out"

sh "$root/scripts/write-release-notes.sh"
npm ci
npm run build
npx playwright install --with-deps chromium

KILN_SCREENSHOT_DIR="$out" npm run test:browser -- tests/dashboard.spec.ts -g 'clicking the version label'

for name in release-notes-header release-notes-open; do
  [ -f "$out/${name}.png" ] || {
    printf 'capture-dashboard-release-notes-proof: missing %s.png\n' "$name" >&2
    exit 1
  }
done

printf 'capture-dashboard-release-notes-proof ok %s\n' "$out"
