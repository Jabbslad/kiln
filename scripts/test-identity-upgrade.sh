#!/bin/bash
# Prove a tag deploy swaps kiln-identity before kiln-app restarts, keeps the
# previous binary, and restores it when the new process does not come up.
# Fixture only. Does not touch a real host.
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
script=$root/deploy/github/install-identity.sh
fail() { printf 'identity-upgrade: %s\n' "$*" >&2; exit 1; }

run_server_upgrade_rehearsal() {
  local prev_tar=$1 next_tar=$2
  local work fx state log package etc opt var libexec
  [ -f "$prev_tar" ] || fail "missing previous server archive: $prev_tar"
  [ -f "$next_tar" ] || fail "missing target server archive: $next_tar"
  work=$(mktemp -d)
  state=$work/stub-state
  log=$work/commands.log
  package=$work/package
  etc=$work/etc/kiln
  opt=$work/opt/kiln
  var=$work/var/lib/kiln
  libexec=$work/usr/local/libexec
  mkdir -p "$work/prev" "$work/next"
  tar -xzf "$prev_tar" -C "$work/prev"
  tar -xzf "$next_tar" -C "$work/next"
  read_release_version_json() {
    tr -d '\n' |
      grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' |
      head -1 |
      sed 's/^.*"\([^"]*\)"$/\1/' |
      sed 's/^v//'
  }
  installed=$(
    tar -xOzf "$prev_tar" release.json 2>/dev/null | read_release_version_json
  )
  target=$(
    tar -xOzf "$next_tar" release.json 2>/dev/null | read_release_version_json
  )
  if [ -z "$installed" ] || [ -z "$target" ]; then
    fail "could not read release.json versions"
  fi
  mkdir -p "$state" "$package/bin" "$etc" "$opt/bin" "$var/runtime" "$var/journal" "$var/image" \
    "$libexec" "$work/run/systemd/system" "$work/sys/fs/cgroup" "$work/dev" \
    "$work/etc/systemd/system" "$work/proc" "$work/stubs"
  cp "$work/next/install.sh" "$package/install.sh"
  chmod 755 "$package/install.sh"
  cp "$root/deploy/operations-journal.max-version" "$package/operations-journal.max-version"
  printf 'ID=ubuntu\nVERSION_ID=24.04\n' > "$work/etc/os-release"
  printf 'MemAvailable:       8388608 kB\n' > "$work/proc/meminfo"
  printf 'cpu memory pids\n' > "$work/sys/fs/cgroup/cgroup.controllers"
  : > "$work/dev/kvm"
  printf 'process\n' > "$state/killmode"
  : > "$log"
  cat > "$work/stubs/systemctl" << EOF
#!/bin/bash
printf '%s\\n' "systemctl \$*" >> "$log"
prop= service=
for arg in "\$@"; do
  case "\$arg" in
    --property=*) prop=\${arg#--property=} ;;
    --value|--no-pager|--quiet) ;;
    show|stop|start|restart|enable|disable|daemon-reload|is-active) ;;
    *) service=\$arg ;;
  esac
done
case "\$1" in
  show)
    case "\$prop" in
      LoadState) printf '%s\\n' loaded ;;
      ActiveState)
        if [ -f "$state/active-\$service" ]; then cat "$state/active-\$service"; else printf '%s\\n' inactive; fi
        ;;
      MainPID) printf '%s\\n' 0 ;;
      KillMode) cat "$state/killmode" ;;
      ExecStart) cat "$state/exec-\$service" ;;
      *) printf '%s\\n' "" ;;
    esac
    ;;
  stop|start)
    for svc in "\${@:2}"; do
      if [ "\$1" = stop ]; then printf '%s\\n' inactive; else printf '%s\\n' active; fi > "$state/active-\$svc"
    done
    ;;
  restart|enable|disable|daemon-reload|is-active) ;;
  *) exit 1 ;;
esac
EOF
  cat > "$work/stubs/curl" << EOF
#!/bin/bash
printf '%s\\n' "curl \$*" >> "$log"
if [ ! -s "$work/curl.seq" ]; then exit 1; fi
line=\$(head -n 1 "$work/curl.seq")
tail -n +2 "$work/curl.seq" > "$work/curl.seq.next"
mv "$work/curl.seq.next" "$work/curl.seq"
[ "\$line" = ok ] || exit 1
printf '%s' '[{"name":"custom","memory_mib":3072,"vcpus":2}]'
EOF
  chmod 755 "$work/stubs/systemctl" "$work/stubs/curl"
  printf '#!/bin/bash\nexit 0\n' > "$work/stubs/systemd-analyze"
  chmod 755 "$work/stubs/systemd-analyze"
  custom=$(printf 'a%.0s' $(seq 1 32))
  printf '{"runtime_dir":"%s/runtime","journal_dir":"%s/journal","socket":"/run/kiln/host.sock","isolation_config":"%s/isolation.json","templates":{"custom":"%s"}}\n' \
    "$var" "$var" "$etc" "$custom" > "$etc/host.json"
  printf '{}\n' > "$etc/isolation.json"
  for name in ca.crt tls.crt tls.key admin.token; do printf 'synthetic-test-only\n' > "$etc/$name"; done
  sqlite3 "$var/journal/operations.sqlite" \
    'PRAGMA journal_mode=DELETE; PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);'
  for name in kiln kiln-runtime kiln-network kiln-host kiln-api; do
    if [ "$name" = kiln ] || [ "$name" = kiln-runtime ] || [ "$name" = kiln-network ]; then
      dest=$opt/bin/$name
      live=$work/prev/bin/$name
      staged=$work/next/bin/$name
    else
      dest=$libexec/$name
      live=$work/prev/bin/$name
      staged=$work/next/bin/$name
    fi
    cp "$live" "$dest"
    chmod 755 "$dest"
    cp "$staged" "$package/bin/$name"
    chmod 755 "$package/bin/$name"
  done
  cp "$work/prev/release.json" "$opt/release.json"
  cp "$work/next/release.json" "$package/release.json"
  host=$libexec/kiln-host
  api=$libexec/kiln-api
  printf '{ path=%s ; argv[]=%s --config %s ; ignore_errors=no ; }\n' "$host" "$host" "$etc/host.json" > "$state/exec-kiln-host.service"
  printf '{ path=%s ; argv[]=%s --listen 100.64.5.6:8443 --host-socket /run/kiln/host.sock --token-file %s --tls-cert %s --tls-key %s ; ignore_errors=no ; }\n' \
    "$api" "$api" "$etc/admin.token" "$etc/tls.crt" "$etc/tls.key" > "$state/exec-kiln-api.service"
  printf 'active\n' > "$state/active-kiln-host.service"
  printf 'active\n' > "$state/active-kiln-api.service"
  printf 'ok\nok\n' > "$work/curl.seq"
  set +e
  KILN_INSTALL_FIXTURE=$work PATH="$work/stubs:$PATH" bash "$package/install.sh" --apply >"$work/out" 2>"$work/err"
  rc=$?
  set -e
  [ "$rc" = 0 ] || {
    cat "$work/err" >&2
    cat "$work/out" >&2
    rm -rf "$work"
    fail "server upgrade rehearsal failed (${installed} -> ${target})"
  }
  rm -rf "$work"
  printf 'server upgrade rehearsal ok (%s -> %s)\n' "$installed" "$target"
}

if [ -n "${KILN_SERVER_UPGRADE_PREV_TAR:-}" ] && [ -n "${KILN_SERVER_UPGRADE_TO_TAR:-}" ]; then
  run_server_upgrade_rehearsal "$KILN_SERVER_UPGRADE_PREV_TAR" "$KILN_SERVER_UPGRADE_TO_TAR"
  exit 0
fi

[ -f "$script" ] || fail "missing $script"

work=
cleanup() { rm -rf "${work:-}"; }
trap cleanup EXIT

write_bin() {
  local path=$1 version=$2
  cat > "$path" << EOF
#!/bin/bash
if [ "\${1:-}" = --version ]; then
  printf 'kiln-identity ${version}\\n'
  exit 0
fi
exit 2
EOF
  chmod 755 "$path"
}

write_slotted() {
  local path=$1 version=$2 marker pad
  marker=$(printf 'KILNREL%s=' SLOT)
  cat > "$path" << 'EOF'
#!/bin/bash
if [ "${1:-}" = --version ]; then
  marker=$(printf 'KILNREL%s=' SLOT)
  slot=$(LC_ALL=C grep -a -o -b -F "$marker" "$0" | head -1)
  offset=${slot%%:*}
  dd if="$0" bs=1 skip=$((offset + 12)) count=32 status=none | tr -d '\000' \
    | awk '{ printf "kiln-identity %s\n", $0 }'
  exit 0
fi
exit 2
EOF
  {
    printf '%s' "$marker"
    printf '%s' "$version"
    pad=$((32 - ${#version}))
    dd if=/dev/zero bs=1 count="$pad" status=none
  } >> "$path"
  chmod 755 "$path"
}

pack_slotted() {
  local dest=$1 version=$2
  local stage archive
  stage=$(mktemp -d)
  mkdir -p "$stage/bin" "$stage/deploy" "$stage/docs"
  write_slotted "$stage/bin/kiln-identity" "$version"
  printf 'unit-from-archive\n' > "$stage/deploy/kiln-identity.service"
  printf 'example-from-archive\n' > "$stage/deploy/identity.example.env"
  printf 'guide-from-archive\n' > "$stage/docs/identity-service.md"
  printf '{"version":"v%s","target":"x86_64-unknown-linux-gnu"}\n' "$version" > "$stage/release.json"
  archive=$dest/kiln-identity-v${version}-x86_64-unknown-linux-gnu.tar.gz
  tar -C "$stage" -czf "$archive" \
    bin/kiln-identity deploy/kiln-identity.service deploy/identity.example.env \
    docs/identity-service.md release.json
  (
    cd "$dest"
    sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256"
  )
  rm -rf "$stage"
}

pack_dashboard() {
  local dest=$1 version=$2
  local stage archive
  stage=$(mktemp -d)
  mkdir -p "$stage/assets"
  printf '<script type="module" crossorigin src="/assets/index-test.js"></script>\n' \
    > "$stage/index.html"
  printf 'js\n' > "$stage/assets/index-test.js"
  archive=$dest/kiln-dashboard-v${version}.tar.gz
  tar -C "$stage" -czf "$archive" index.html assets/index-test.js
  (
    cd "$dest"
    sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256"
  )
  rm -rf "$stage"
}

pack_identity() {
  local dest=$1 version=$2 bytes=$3
  local stage archive
  stage=$(mktemp -d)
  mkdir -p "$stage/bin" "$stage/deploy" "$stage/docs"
  write_bin "$stage/bin/kiln-identity" "$version"
  printf '%s\n' "$bytes" >> "$stage/bin/kiln-identity"
  printf 'unit-from-archive\n' > "$stage/deploy/kiln-identity.service"
  printf 'example-from-archive\n' > "$stage/deploy/identity.example.env"
  printf 'guide-from-archive\n' > "$stage/docs/identity-service.md"
  printf '{"version":"v%s","target":"x86_64-unknown-linux-gnu"}\n' "$version" > "$stage/release.json"
  archive=$dest/kiln-identity-v${version}-x86_64-unknown-linux-gnu.tar.gz
  tar -C "$stage" -czf "$archive" \
    bin/kiln-identity deploy/kiln-identity.service deploy/identity.example.env \
    docs/identity-service.md release.json
  (
    cd "$dest"
    sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256"
  )
  rm -rf "$stage"
}

prepare() {
  if [ -n "${work:-}" ]; then rm -rf "$work"; fi
  work=$(mktemp -d)
  fx=$work/root
  state=$work/state
  log=$work/commands.log
  deploy=$work/deploy
  live=$fx/usr/local/libexec/kiln-identity
  mkdir -p "$fx/usr/local/libexec" "$fx/opt/kiln" "$fx/proc" "$fx/run/lock" \
    "$fx/etc/kiln-identity" "$fx/etc/kiln" "$fx/var/lib/kiln-identity" \
    "$fx/etc/systemd/system" "$state" "$deploy" "$work/stubs"
  printf 'unit-sentinel\n' > "$fx/etc/systemd/system/kiln-identity.service"
  printf 'env-sentinel\n' > "$fx/etc/kiln-identity/service.env"
  printf 'enroll-sentinel\n' > "$fx/etc/kiln/identity.json"
  printf 'db-sentinel\n' > "$fx/var/lib/kiln-identity/identity.db"
  printf 'active\n' > "$state/active"
  printf '4300\n' > "$state/pid"
  mkdir -p "$fx/proc/4300"
  ln -s "$live" "$fx/proc/4300/exe"
  : > "$log"
  cat > "$work/stubs/systemctl" << EOF
#!/bin/bash
printf '%s\\n' "systemctl \$*" >> "$log"
prop= service=
for arg in "\$@"; do
  case "\$arg" in
    --property=*|--value|--no-pager|--quiet) ;;
    -p) shift_next=1 ;;
    show|stop|start|restart|enable|disable|daemon-reload|is-active) ;;
    *)
      if [ "\${arg#-p}" != "\$arg" ]; then
        prop=\${arg#-p}
      else
        service=\$arg
      fi
      ;;
  esac
done
# -p PROP is two arguments. Re-scan.
prop=
args=("\$@")
i=0
while [ "\$i" -lt "\$#" ]; do
  cur=\${args[\$i]}
  if [ "\$cur" = -p ]; then
    i=\$((i + 1))
    prop=\${args[\$i]}
  fi
  i=\$((i + 1))
done
case "\$1" in
  show)
    case "\$prop" in
      ActiveState) cat "$state/active" ;;
      MainPID) cat "$state/pid" ;;
      *) printf '%s\\n' inactive ;;
    esac
    ;;
  restart)
    old=\$(tr -d '[:space:]' < "$state/pid")
    case "\$old" in ''|*[!0-9]*) old=0 ;; esac
    next=\$((old + 1))
    printf '%s\\n' "\$next" > "$state/pid"
    printf 'active\\n' > "$state/active"
    rm -rf "$fx/proc/\$old"
    mkdir -p "$fx/proc/\$next"
    ln -s "$live" "$fx/proc/\$next/exe"
    ;;
  *) exit 1 ;;
esac
EOF
  cat > "$work/stubs/curl" << EOF
#!/bin/bash
printf '%s\\n' "curl \$*" >> "$log"
url=\${@: -1}
out= hdr=
prev=
for arg in "\$@"; do
  if [ "\$prev" = --output ]; then out=\$arg; fi
  if [ "\$prev" = -D ]; then hdr=\$arg; fi
  prev=\$arg
done
case "\$url" in
  http://127.0.0.1:8091/login)
    if [ -f "$state/fail-loopback" ]; then
      printf '500'
      exit 0
    fi
    if [ -n "\$out" ]; then printf 'Light the way in.\\n' > "\$out"; fi
    printf '200'
    ;;
  https://dark-forge.dev/login)
    if [ -f "$state/fail-public" ]; then
      printf '500'
      exit 0
    fi
    if [ -n "\$out" ]; then printf '<h1>Light the way in.</h1>\\n' > "\$out"; fi
    printf '200'
    ;;
  https://app.dark-forge.dev/login)
    if [ -n "\$hdr" ]; then
      if [ -f "$state/fail-public" ]; then
        printf 'HTTP/1.1 200\\r\\n\\r\\n' > "\$hdr"
      else
        printf 'HTTP/1.1 303\\r\\nLocation: https://dark-forge.dev/oauth/server?redirect_uri=https%%3A%%2F%%2Fapp.dark-forge.dev%%2Foauth%%2Fcallback&code_challenge_method=S256\\r\\n\\r\\n' > "\$hdr"
      fi
    fi
    if [ -f "$state/fail-public" ]; then printf '200'; else printf '303'; fi
    ;;
  *)
    printf 'curl stub: unexpected %s\\n' "\$url" >&2
    exit 1
    ;;
esac
EOF
  chmod 755 "$work/stubs/systemctl" "$work/stubs/curl"
}

run_id() {
  set +e
  PATH="$work/stubs:$PATH" KILN_IDENTITY_FIXTURE=$fx bash "$script" "$deploy" >"$work/out" 2>"$work/err"
  rc=$?
  set -e
}

sentinel_inodes() {
  stat -c '%i %n' \
    "$fx/etc/systemd/system/kiln-identity.service" \
    "$fx/etc/kiln-identity/service.env" \
    "$fx/etc/kiln/identity.json" \
    "$fx/var/lib/kiln-identity/identity.db"
}

assert_sentinels() {
  local now
  now=$(sentinel_inodes)
  [ "$now" = "$sentinels" ] || fail "sentinel changed: $now"
  grep -qx 'unit-sentinel' "$fx/etc/systemd/system/kiln-identity.service"
  grep -qx 'env-sentinel' "$fx/etc/kiln-identity/service.env"
  grep -qx 'enroll-sentinel' "$fx/etc/kiln/identity.json"
  grep -qx 'db-sentinel' "$fx/var/lib/kiln-identity/identity.db"
}

# Swap, backup, restart, then a second call that does not restart.
prepare
write_bin "$live" 0.4.15
printf 'old-bytes\n' >> "$live"
pack_identity "$deploy" 0.4.16 'new-bytes'
sentinels=$(sentinel_inodes)
run_id
[ "$rc" = 0 ] || fail "swap failed: $(cat "$work/err")"
grep -q 'identity 0.4.16' "$work/out" || fail "missing version line: $(cat "$work/out")"
[ "$(grep -c 'identity probe version: match 0.4.16' "$work/out" || true)" = 1 ] \
  || fail "version probe: $(cat "$work/out")"
[ "$(grep -c 'identity probe loopback: status=200 phrase=found' "$work/out" || true)" = 1 ] \
  || fail "loopback probe: $(cat "$work/out")"
[ "$(grep -c 'identity probe public: status=200 phrase=found' "$work/out" || true)" = 1 ] \
  || fail "public probe: $(cat "$work/out")"
[ "$(grep -c 'identity probe app: status=303 location_oauth_server=yes code_challenge_method=S256' "$work/out" || true)" = 1 ] \
  || fail "app probe: $(cat "$work/out")"
if grep -q 'redirect_uri' "$work/out"; then fail "probe printed a redirect"; fi
installed=$(PATH="$work/stubs:$PATH" "$live" --version)
[ "$installed" = "kiln-identity 0.4.16" ] || fail "live version $installed"
grep -q 'new-bytes' "$live"
backup=$(find "$fx/opt/kiln/upgrades" -type f -name kiln-identity ! -path '*/identity-in-progress/*')
[ -n "$backup" ] || fail "previous binary was not kept"
grep -q 'old-bytes' "$backup"
[ ! -d "$fx/opt/kiln/upgrades/identity-in-progress" ] || fail "marker left behind"
grep -q 'systemctl restart kiln-identity.service' "$log" || fail "did not restart identity"
if grep -q 'kiln-app' "$log"; then fail "touched kiln-app"; fi
loop_line=$(grep -n 'http://127.0.0.1:8091/login' "$log" | head -1 | cut -d: -f1)
restart_line=$(grep -n 'systemctl restart kiln-identity.service' "$log" | head -1 | cut -d: -f1)
if [ -z "$loop_line" ] || [ -z "$restart_line" ] || [ "$restart_line" -ge "$loop_line" ]; then
  fail "probe ran before restart"
fi
app_line=$(grep -n 'https://app.dark-forge.dev/login' "$log" | head -1 | cut -d: -f1)
if [ -z "$app_line" ] || [ "$loop_line" -ge "$app_line" ]; then
  fail "public login was not probed"
fi
assert_sentinels
mode=$(stat -c '%a' "$(dirname "$backup")")
[ "$mode" = 700 ] || fail "backup dir mode $mode"
cp "$log" "$work/first.log"
: > "$log"
run_id
[ "$rc" = 0 ] || fail "second call failed: $(cat "$work/err")"
restarts=$(grep -c 'systemctl restart kiln-identity.service' "$work/first.log" || true)
[ "$restarts" = 1 ] || fail "first call restarted $restarts times"
if grep -q 'systemctl restart' "$log"; then fail "second call restarted identity"; fi
grep -q 'identity unchanged 0.4.16' "$work/out" || fail "second call was not unchanged: $(cat "$work/out")"
if grep -q 'curl ' "$log"; then fail "second call probed"; fi
assert_sentinels

# A failed public login after the swap puts the previous binary back.
prepare
write_bin "$live" 0.4.15
printf 'old-bytes\n' >> "$live"
pack_identity "$deploy" 0.4.16 'new-bytes'
printf '1\n' > "$state/fail-public"
run_id
[ "$rc" != 0 ] || fail "public failure should fail the swap"
grep -q 'old-bytes' "$live" || fail "public failure left the new binary"
grep -q 'identity: restored' "$work/err" || fail "missing restore line: $(cat "$work/err")"
[ ! -d "$fx/opt/kiln/upgrades/identity-in-progress" ] || fail "marker left after restore"

# A failed loopback probe restores and does not leave the new binary running.
prepare
write_bin "$live" 0.4.15
printf 'old-bytes\n' >> "$live"
pack_identity "$deploy" 0.4.16 'new-bytes'
printf '1\n' > "$state/fail-loopback"
run_id
[ "$rc" != 0 ] || fail "loopback failure should fail"
grep -q 'old-bytes' "$live" || fail "loopback failure left the new binary"
[ "$(grep -c 'identity probe loopback:' "$work/out" || true)" = 1 ] \
  || fail "loopback logged every retry: $(cat "$work/out")"
grep -q 'identity probe loopback: status=500 phrase=missing' "$work/out" \
  || fail "loopback result: $(cat "$work/out")"
if grep -q 'redirect_uri' "$work/out"; then fail "probe printed a redirect"; fi

# Already on the package: a later public failure does not roll the binary back.
prepare
write_bin "$live" 0.4.16
printf 'new-bytes\n' >> "$live"
pack_identity "$deploy" 0.4.16 'new-bytes'
run_id
[ "$rc" = 0 ] || fail "matching binary should pass: $(cat "$work/err")"
if grep -q 'systemctl restart' "$log"; then fail "matching binary restarted"; fi
grep -q 'identity unchanged 0.4.16' "$work/out" || fail "matching binary was probed: $(cat "$work/out")"
printf '1\n' > "$state/fail-public"
: > "$log"
run_id
[ "$rc" = 0 ] || fail "unchanged binary should skip a later public failure: $(cat "$work/err")"
if grep -q 'curl ' "$log"; then fail "unchanged binary probed again"; fi
grep -q 'new-bytes' "$live" || fail "unchanged binary was replaced"

# A version slot is the only difference. The live binary stays, and probes do not run.
prepare
write_slotted "$live" 0.4.19
pack_slotted "$deploy" 0.4.20
run_id
[ "$rc" = 0 ] || fail "slot-only upgrade should skip: $(cat "$work/err")"
grep -q 'identity unchanged 0.4.20' "$work/out" || fail "slot-only was installed: $(cat "$work/out")"
if grep -q 'systemctl restart' "$log"; then fail "slot-only restarted"; fi
if grep -q 'curl ' "$log"; then fail "slot-only probed"; fi
live_version=$(PATH="$work/stubs:$PATH" "$live" --version)
[ "$live_version" = "kiln-identity 0.4.19" ] || fail "slot-only replaced the binary: $live_version"

# Downgrade is refused before a restart. 0.4.4 to 0.4.10 is allowed.
prepare
write_bin "$live" 0.4.16
printf 'current\n' >> "$live"
pack_identity "$deploy" 0.4.10 'older'
run_id
[ "$rc" != 0 ] || fail "downgrade should fail"
grep -q 'current' "$live" || fail "downgrade wrote the binary"
if grep -q 'systemctl restart' "$log"; then fail "downgrade restarted"; fi
prepare
write_bin "$live" 0.4.4
printf 'old\n' >> "$live"
pack_identity "$deploy" 0.4.10 'ten'
run_id
[ "$rc" = 0 ] || fail "0.4.4 to 0.4.10 should install: $(cat "$work/err")"
grep -q 'ten' "$live" || fail "0.4.10 bytes missing"

# Bad checksum, a checksum that names another asset, a missing archive, an unsafe member.
prepare
write_bin "$live" 0.4.15
pack_identity "$deploy" 0.4.16 'new-bytes'
archive=$(find "$deploy" -name 'kiln-identity-*.tar.gz' ! -name '*.sha256')
printf '0000000000000000000000000000000000000000000000000000000000000000  %s\n' "$(basename "$archive")" > "${archive}.sha256"
run_id
[ "$rc" != 0 ] || fail "bad checksum should fail"
if grep -q 'systemctl restart' "$log"; then fail "bad checksum restarted"; fi
prepare
write_bin "$live" 0.4.15
pack_identity "$deploy" 0.4.16 'new-bytes'
archive=$(find "$deploy" -name 'kiln-identity-*.tar.gz' ! -name '*.sha256')
digest=$(awk 'NR==1 { print $1 }' "${archive}.sha256")
printf '%s  kiln-server-v0.4.16-x86_64-unknown-linux-gnu.tar.gz\n' "$digest" > "${archive}.sha256"
run_id
[ "$rc" != 0 ] || fail "checksum for another asset should fail"
grep -q 'unexpected checksum file' "$work/err" || fail "missing checksum rejection: $(cat "$work/err")"
prepare
write_bin "$live" 0.4.15
run_id
[ "$rc" != 0 ] || fail "missing archive should fail"
prepare
write_bin "$live" 0.4.15
stage=$(mktemp -d)
mkdir -p "$stage/bin"
write_bin "$stage/bin/kiln-identity" 0.4.16
printf '{"version":"v0.4.16","target":"x86_64-unknown-linux-gnu"}\n' > "$stage/release.json"
archive=$deploy/kiln-identity-v0.4.16-x86_64-unknown-linux-gnu.tar.gz
tar -C "$stage" -czf "$archive" --transform 's,^bin/kiln-identity,../kiln-identity,' bin/kiln-identity release.json
( cd "$deploy" && sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256" )
rm -rf "$stage"
run_id
[ "$rc" != 0 ] || fail "unsafe archive should fail"
grep -q 'archive path is not safe' "$work/err" || fail "missing path rejection: $(cat "$work/err")"
if grep -q 'systemctl restart' "$log"; then fail "unsafe archive restarted"; fi

# An unfinished marker whose live bytes already match is retired, not restored.
prepare
write_bin "$live" 0.4.16
printf 'new-bytes\n' >> "$live"
pack_identity "$deploy" 0.4.16 'new-bytes'
mkdir -p "$fx/opt/kiln/upgrades/identity-in-progress"
write_bin "$fx/opt/kiln/upgrades/identity-in-progress/kiln-identity" 0.4.15
printf 'old-bytes\n' >> "$fx/opt/kiln/upgrades/identity-in-progress/kiln-identity"
run_id
[ "$rc" = 0 ] || fail "finished marker should retire: $(cat "$work/err")"
grep -q 'new-bytes' "$live" || fail "finished marker restored over the new binary"
[ ! -d "$fx/opt/kiln/upgrades/identity-in-progress" ] || fail "finished marker was not retired"
found=$(find "$fx/opt/kiln/upgrades" -type f -name kiln-identity)
grep -q 'old-bytes' "$found" || fail "retired marker lost the previous binary"

# The deploy wrapper downloads the identity asset and will not SSH when it is bad.
# install-remote.sh upgrades identity once, before the server installer.
order=$(awk '
  /run_identity\(\)/ { next }
  /run_identity/ { calls[++n] = NR }
  /before=\$\(stamp\)/ { stamp = NR }
  /package\/install.sh/ && /--apply/ && !apply { apply = NR }
  END {
    if (n != 1 || !stamp || !apply) exit 1
    if (calls[1] < stamp && stamp < apply) print "ok"
  }
' "$root/deploy/github/install-remote.sh")
[ "$order" = ok ] || fail "identity swap is not around the server install"

deploy_text=$(cat "$root/deploy/github/deploy.sh")
case "$deploy_text" in
  *kiln-identity-*) ;;
  *) fail "deploy.sh does not fetch the identity archive" ;;
esac
case "$deploy_text" in
  *kiln-dashboard-*) ;;
  *) fail "deploy.sh does not fetch the dashboard archive" ;;
esac
for banned in service.env github.secret google.secret signing.pem identity.json identity.db; do
  if grep -q "$banned" "$script" || grep -q "$banned" "$root/deploy/github/deploy.sh"; then
    fail "deploy path names $banned"
  fi
done
if grep -q 'systemctl restart kiln-identity' "$root/deploy/install.sh"; then
  fail "server installer restarts identity"
fi
grep -q 'does not replace kiln-identity' "$root/deploy/install.sh" || fail "installer contract comment missing"

prepare_deploy() {
  local bad=${1:-}
  prepare
  files=$work/files
  mkdir -p "$files"
  write_bin "$work/server-bin" 0.4.16
  tar -C "$work" -czf "$files/kiln-server-v0.4.16-x86_64-unknown-linux-gnu.tar.gz" server-bin
  pack_identity "$files" 0.4.16 'pkg'
  pack_dashboard "$files" 0.4.16
  (
    cd "$files"
    sha256sum kiln-server-v0.4.16-x86_64-unknown-linux-gnu.tar.gz > kiln-server-v0.4.16-x86_64-unknown-linux-gnu.tar.gz.sha256
  )
  if [ "$bad" = identity ]; then
    id=$files/kiln-identity-v0.4.16-x86_64-unknown-linux-gnu.tar.gz
    printf '0000000000000000000000000000000000000000000000000000000000000000  %s\n' "$(basename "$id")" > "${id}.sha256"
  fi
  if [ "$bad" = missing ]; then
    rm -f "$files"/kiln-identity-*
  fi
  cat > "$work/stubs/curl" << EOF
#!/bin/bash
printf '%s\\n' "\$*" >> "$log"
url=\${@: -1}
out=
prev=
for arg in "\$@"; do
  if [ "\$prev" = --output ]; then out=\$arg; fi
  prev=\$arg
done
name=\$(basename "\$url")
if [ ! -f "$files/\$name" ]; then
  printf 'curl: missing %s\\n' "\$name" >&2
  exit 1
fi
cp "$files/\$name" "\$out"
EOF
  cat > "$work/ssh.sh" << EOF
#!/bin/bash
printf '%s\\n' "\$*" >> "$work/ssh.log"
if [ "\$1" = scp ]; then
  mkdir -p -- "\$(dirname -- "\$3")"
  cp -- "\$2" "\$3"
  exit 0
fi
if [ "\$1" = ssh ]; then
  shift
  [ "\${1:-}" = -- ] && shift
  if [ "\${1:-}" = mktemp ]; then
    shift
    mktemp "\$@"
    exit 0
  fi
  if [ "\${1:-}" = rm ]; then
    shift
    rm "\$@"
    exit 0
  fi
  exit 0
fi
exit 1
EOF
  chmod 755 "$work/stubs/curl" "$work/ssh.sh"
  : > "$work/ssh.log"
}

run_deploy() {
  set +e
  env -u GITHUB_EVENT_NAME -u GITHUB_REF -u GITHUB_REPOSITORY -u KILN_ADDRESS \
    PATH="$work/stubs:$PATH" KILN_RELEASE_TAG=v0.4.16 KILN_SSH_SH="$work/ssh.sh" \
    bash "$root/deploy/github/deploy.sh" >"$work/out" 2>"$work/err"
  rc=$?
  set -e
}

prepare_deploy
run_deploy
[ "$rc" = 0 ] || fail "deploy wrapper failed: $(cat "$work/err")"
grep -q 'deployed v0.4.16' "$work/out" || fail "missing deployed line"
grep -q 'kiln-identity-v0.4.16-x86_64-unknown-linux-gnu.tar.gz' "$log" || fail "identity asset was not downloaded"
grep -q 'kiln-identity-v0.4.16-x86_64-unknown-linux-gnu.tar.gz.sha256' "$log" || fail "identity checksum was not downloaded"
grep -q 'install-identity.sh' "$work/ssh.log" || fail "identity installer was not copied"
prepare_deploy identity
run_deploy
[ "$rc" != 0 ] || fail "bad identity checksum should fail the deploy"
grep -q 'NOT match' "$work/err" || fail "bad identity checksum stderr: $(cat "$work/err")"
if grep -q 'scp ' "$work/ssh.log"; then fail "bad checksum reached scp"; fi
prepare_deploy missing
run_deploy
[ "$rc" != 0 ] || fail "missing identity asset should fail the deploy"
if [ -s "$work/ssh.log" ]; then fail "missing identity asset reached ssh"; fi

rm -rf "$work"
trap - EXIT
printf 'journal v1 upgrade checks\n'
cargo test -p kiln-server --lib v1_upgrade_leaves_legacy_finished_at_null --locked
cargo test -p kiln-server --lib v1_running_box_upgrade_keeps_null_timing_and_passes_self_check --locked
cargo test -p kiln-server --lib v2_finish_records_measured_launch_duration --locked
printf 'identity upgrade checks passed\n'
