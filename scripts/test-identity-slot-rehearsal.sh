#!/bin/bash
# Rehearse identity slot-only upgrade on stamped release artifacts (Linux x86-64).
set -euo pipefail

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
script=$root/deploy/github/install-identity.sh
stamp_sh=$root/scripts/stamp-release-version.sh

fail() {
  printf 'identity-slot-rehearsal: %s\n' "$*" >&2
  exit 1
}

dist=${1:?usage: test-identity-slot-rehearsal.sh DIST TAG}
tag=${2:?usage: test-identity-slot-rehearsal.sh DIST TAG}
old=${3:-0.4.29}

printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' \
  || fail "tag ${tag}"
want=${tag#v}

[ -d "$dist" ] || fail 'dist is not a directory'
[ -f "$script" ] || fail "missing $script"
[ -f "$stamp_sh" ] || fail "missing $stamp_sh"

archive="$dist/kiln-identity-${tag}-x86_64-unknown-linux-gnu.tar.gz"
test -f "$archive" || fail "missing identity archive for ${tag}"
test -f "${archive}.sha256" || fail 'missing identity checksum'

work=
cleanup() {
  rm -rf "${work:-}"
}
trap cleanup EXIT

work=$(mktemp -d)
fx=$work/root
deploy=$work/deploy
log=$work/commands.log
live=$fx/usr/local/libexec/kiln-identity
state=$work/state
mkdir -p "$fx/usr/local/libexec" "$fx/opt/kiln" "$fx/proc" "$fx/run/lock" "$deploy" "$state" "$work/stubs"
printf 'active\n' >"$state/active"
printf '4300\n' >"$state/pid"
mkdir -p "$fx/proc/4300"
ln -s "$live" "$fx/proc/4300/exe"

cat >"$work/stubs/systemctl" <<EOF
#!/bin/sh
printf '%s\n' "systemctl \$*" >> "$log"
prop=
args="\$@"
while [ "\$#" -gt 0 ]; do
  case "\$1" in
    -p) shift; prop=\$1 ;;
  esac
  shift
done
case "\$prop" in
  ActiveState) cat "$state/active" ;;
  MainPID) cat "$state/pid" ;;
  *) printf '%s\n' inactive ;;
esac
EOF
cat >"$work/stubs/curl" <<EOF
#!/bin/sh
printf '%s\n' "curl \$*" >> "$log"
url=\${@: -1}
out=
prev=
for arg in "\$@"; do
  if [ "\$prev" = --output ]; then out=\$arg; fi
  prev=\$arg
done
case "\$url" in
  http://127.0.0.1:8091/login)
    [ -n "\$out" ] && printf 'Light the way in.\\n' > "\$out"
    printf '200'
    ;;
  https://dark-forge.dev/login)
    [ -n "\$out" ] && printf 'Light the way in.\\n' > "\$out"
    printf '200'
    ;;
  https://app.dark-forge.dev/login)
    printf '303'
    ;;
  *) exit 1 ;;
esac
EOF
chmod 755 "$work/stubs/systemctl" "$work/stubs/curl"

cp "$archive" "$deploy/"
cp "${archive}.sha256" "$deploy/"

stage=$work/extract
mkdir -p "$stage"
tar -xzf "$archive" -C "$stage" bin/kiln-identity
cp "$stage/bin/kiln-identity" "$live"
chmod 755 "$live"
sh "$stamp_sh" "$live" "$old"

before=$work/before
cp "$live" "$before"
before_ver=$("$live" --version) || fail 'stale identity --version failed'
case $before_ver in
  "kiln-identity $old") ;;
  *) fail "stale identity reports ${before_ver}, want kiln-identity ${old}" ;;
esac

: >"$log"
set +e
PATH="$work/stubs:$PATH" KILN_IDENTITY_FIXTURE=$fx bash "$script" "$deploy" >"$work/out" 2>"$work/err"
rc=$?
set -e
[ "$rc" = 0 ] || fail "install failed: $(cat "$work/err" 2>/dev/null)"

grep -q "identity version ${want}" "$work/out" \
  || fail "missing identity version line: $(cat "$work/out")"
if grep -q 'systemctl restart kiln-identity.service' "$log"; then
  fail 'slot-only upgrade restarted identity'
fi
if grep -q 'curl ' "$log"; then
  fail 'slot-only upgrade probed login'
fi

after_ver=$("$live" --version) || fail 'updated identity --version failed'
[ "$after_ver" = "kiln-identity $want" ] \
  || fail "live reports ${after_ver}, want kiln-identity ${want}"

tar -xOzf "$archive" bin/kiln-identity >"$work/package-bin"
cmp -s "$live" "$work/package-bin" \
  || fail 'live bytes differ from the release identity binary after slot update'

cmp -s "$before" "$work/package-bin" \
  && fail 'stale live was identical to the package; test must use a stale slot only'

printf 'identity slot rehearsal %s\n' "$tag"
