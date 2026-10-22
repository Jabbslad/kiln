#!/bin/sh
# Keep deploy/install.sh and kiln-server journal.rs aligned on the max schema version.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-operations-journal-version-sync: %s\n' "$*" >&2
  exit 1
}

pin=$root/deploy/operations-journal.max-version
journal=$root/crates/kiln-server/src/journal.rs
install=$root/deploy/install.sh

[ -f "$pin" ] || fail "missing $pin"
max=$(tr -d '[:space:]' <"$pin")
case $max in
  ''|*[!0-9]*) fail "pin must be a decimal version" ;;
esac

grep -q 'operations-journal.max-version' "$install" \
  || fail 'install.sh must read deploy/operations-journal.max-version'
grep -q 'operations_journal_max_version' "$install" \
  || fail 'install.sh must define operations_journal_max_version'
grep -q 'operations_journal_version_supported' "$install" \
  || fail 'install.sh must define operations_journal_version_supported'
grep -q 'found operation journal schema version' "$install" \
  || fail 'install.sh must report unsupported journal schema versions'

grep -q 'deploy/operations-journal.max-version' "$journal" \
  || fail 'journal.rs must include deploy/operations-journal.max-version'
awk '
  /^const SCHEMA_VERSION: u32 = operations_journal_max_version\(\);$/ { saw = 1 }
  END { exit !saw }
' "$journal" || fail 'journal.rs must set SCHEMA_VERSION from operations_journal_max_version()'

grep -q "operations-journal.max-version" "$root/xtask/src/release.rs" \
  || fail 'server release must ship operations-journal.max-version'

printf 'operations journal max version sync ok (%s)\n' "$max"
