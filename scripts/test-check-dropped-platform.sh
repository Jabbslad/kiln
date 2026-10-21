#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check-dropped-platform: %s\n' "$*" >&2
  exit 1
}

docs=$(awk '/^  docs-check:$/,/^  feature-map-guard:$/' .github/workflows/build.yml)
server=$(awk '/^  server-check:$/,/^  reuse:$/' .github/workflows/build.yml)
printf '%s\n' "$docs" | grep -q 'sh scripts/test-check-dropped-platform.sh' \
  || fail 'docs-check does not run the platform check'
printf '%s\n' "$server" | grep -q 'sh scripts/test-check-dropped-platform.sh' \
  || fail 'server-check does not run the platform check'
printf '%s\n' "$docs" | grep -q 'needs: changes' \
  || fail 'docs-check is not a required dependency'
printf '%s\n' "$server" | grep -q 'needs: \[changes, reuse\]' \
  || fail 'server-check is not a required dependency'
grep -q 'needs: \[changes, docs-check, feature-map-guard, server-check, client, dashboard\]' \
  .github/workflows/build.yml \
  || fail 'required does not depend on docs-check and server-check'

sh "$root/scripts/check-dropped-platform.sh"

work=$(mktemp -d)
out=$(mktemp)
err=$(mktemp)
trap 'rm -rf "$work" "$out" "$err"' EXIT
mkdir -p "$work/scripts" "$work/docs/superpowers/plans" "$work/notes"
cat > "$work/scripts/dropped-platform-allowlist.txt" << 'EOF'
docs/releases.md
docs/superpowers/
EOF
printf '%s\n' 'v0.4.8 Windows stamp failed' > "$work/docs/releases.md"
printf '%s\n' 'native Windows client' > "$work/docs/superpowers/plans/old.md"
printf '%s\n' 'Linux and macOS only' > "$work/notes/clean.md"
printf '%s\n' 'buf.windows(4); sys.executable' > "$work/notes/method.rs"
sh "$root/scripts/check-dropped-platform.sh" "$work"

plant() {
  label=$1
  text=$2
  printf '%s\n' "$text" > "$work/notes/planted.md"
  if sh "$root/scripts/check-dropped-platform.sh" "$work" > "$out" 2> "$err"; then
    fail "planted ${label} reference was accepted"
  fi
  grep -q 'notes/planted.md' "$err" || fail "missing planted ${label} path: $(cat "$err")"
  rm -f "$work/notes/planted.md"
}

plant windows 'Install the Windows client'
plant exe 'curl -fO kiln.exe'
plant powershell 'Run the installer in PowerShell'
sh "$root/scripts/check-dropped-platform.sh" "$work"

printf 'dropped platform check passed\n'
