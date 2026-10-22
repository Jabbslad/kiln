#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check-installed-versions: %s\n' "$*" >&2
  exit 1
}

check=$root/deploy/github/check-installed-versions.sh
[ -f "$check" ] || fail "missing $check"
chmod +x "$check"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

write_bin() {
  path=$1
  name=$2
  version=$3
  cat >"$path" <<EOF
#!/bin/sh
if [ "\${1:-}" = --version ]; then
  printf '%s %s\n' '$name' '$version'
  exit 0
fi
exit 2
EOF
  chmod 755 "$path"
}

fx=$work/root
mkdir -p "$fx/opt/kiln/bin" "$fx/usr/local/libexec"
write_bin "$fx/opt/kiln/bin/kiln" kiln 0.4.20
write_bin "$fx/opt/kiln/bin/kiln-runtime" kiln-runtime 0.4.20
write_bin "$fx/usr/local/libexec/kiln-host" kiln-server 0.4.20
write_bin "$fx/usr/local/libexec/kiln-api" kiln-server 0.4.20
write_bin "$fx/usr/local/libexec/kiln-identity" kiln-identity 0.4.20

KILN_VERSION_CHECK_FIXTURE=$fx bash "$check" v0.4.20 >"$work/out" \
  || fail "fixture check failed: $(cat "$work/out")"
grep -q 'installed versions v0.4.20' "$work/out" || fail "missing installed line: $(cat "$work/out")"

write_bin "$fx/usr/local/libexec/kiln-identity" kiln-identity 0.4.19
if KILN_VERSION_CHECK_FIXTURE=$fx bash "$check" v0.4.20 >/dev/null 2>&1; then
  fail 'stale identity version should fail'
fi

dist=$work/dist
mkdir -p "$dist/server/bin" "$dist/identity/bin"
write_bin "$dist/server/bin/kiln" kiln 0.4.21
write_bin "$dist/server/bin/kiln-runtime" kiln-runtime 0.4.21
write_bin "$dist/server/bin/kiln-host" kiln-server 0.4.21
write_bin "$dist/server/bin/kiln-api" kiln-server 0.4.21
write_bin "$dist/identity/bin/kiln-identity" kiln-identity 0.4.21
tar -C "$dist/server" -czf "$dist/kiln-server-v0.4.21-x86_64-unknown-linux-gnu.tar.gz" \
  bin/kiln bin/kiln-runtime bin/kiln-host bin/kiln-api
tar -C "$dist/identity" -czf "$dist/kiln-identity-v0.4.21-x86_64-unknown-linux-gnu.tar.gz" \
  bin/kiln-identity

KILN_VERSION_CHECK_DIST="$dist" bash "$check" v0.4.21 >"$work/dist-out" \
  || fail "dist check failed: $(cat "$work/dist-out")"
grep -q 'packaged versions v0.4.21' "$work/dist-out" \
  || fail "missing packaged line: $(cat "$work/dist-out")"

printf 'check-installed-versions tests ok\n'
