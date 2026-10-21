#!/bin/bash
# Upgrade publishes /etc/kiln/login.token only when that path is missing.
# Fixture only. Does not touch a real host.
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
install=$root/deploy/install.sh
fail() { printf 'login-token: %s\n' "$*" >&2; exit 1; }

pid_of() { tr -d '[:space:]' < "$state/pid-$1"; }

prepare() {
  if [ -n "${work:-}" ]; then rm -rf "$work"; fi
  work=$(mktemp -d)
  state=$work/stub-state
  log=$work/commands.log
  package=$work/package
  etc=$work/etc/kiln
  opt=$work/opt/kiln
  var=$work/var/lib/kiln
  libexec=$work/usr/local/libexec
  mkdir -p "$state" "$package/bin" "$etc" "$opt/bin" "$var/runtime" "$var/journal" "$var/image" \
    "$libexec" "$work/run/systemd/system" "$work/sys/fs/cgroup" "$work/dev" \
    "$work/etc/systemd/system" "$work/proc" "$work/stubs"
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
      LoadState) printf '%s\\n' not-found ;;
      ActiveState)
        if [ -f "$state/active-\$service" ]; then cat "$state/active-\$service"; else printf '%s\\n' inactive; fi
        ;;
      MainPID)
        if [ -f "$state/pid-\$service" ]; then cat "$state/pid-\$service"; else printf '%s\\n' 0; fi
        ;;
      KillMode) cat "$state/killmode" ;;
      ExecStart) cat "$state/exec-\$service" ;;
      *) printf '%s\\n' "" ;;
    esac
    ;;
  restart)
    old=0
    if [ -f "$state/pid-\$service" ]; then
      old=\$(tr -d '[:space:]' < "$state/pid-\$service")
    fi
    case "\$old" in ''|*[!0-9]*) old=0 ;; esac
    printf '%s\\n' "\$((old + 1))" > "$state/pid-\$service"
    printf '%s\\n' active > "$state/active-\$service"
    ;;
  stop|start)
    for svc in "\${@:2}"; do
      if [ "\$1" = stop ]; then printf '%s\\n' inactive; else printf '%s\\n' active; fi > "$state/active-\$svc"
    done
    ;;
  is-active) printf '%s\\n' inactive ;;
  enable|disable|daemon-reload) ;;
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
  printf '#!/bin/bash\nexit 0\n' > "$work/stubs/systemd-analyze"
  chmod 755 "$work/stubs/systemctl" "$work/stubs/curl" "$work/stubs/systemd-analyze"
  cp "$install" "$package/install.sh"
  chmod 755 "$package/install.sh"
  custom=$(printf 'a%.0s' $(seq 1 32))
  printf '{"runtime_dir":"%s/runtime","journal_dir":"%s/journal","socket":"/run/kiln/host.sock","isolation_config":"%s/isolation.json","templates":{"custom":"%s"}}\n' \
    "$var" "$var" "$etc" "$custom" > "$etc/host.json"
  printf '{}\n' > "$etc/isolation.json"
  for name in ca.crt tls.crt tls.key admin.token; do printf 'synthetic-test-only\n' > "$etc/$name"; done
  printf '%s\n' '{"version":1,"issuer":"https://identity.example","owner_id":"owner","server_id":"server"}' > "$etc/identity.json"
  chmod 640 "$etc/identity.json"
  sqlite3 "$var/journal/operations.sqlite" 'PRAGMA journal_mode=DELETE; PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);' >/dev/null
  host=$libexec/kiln-host
  api=$libexec/kiln-api
  printf '{ path=%s ; argv[]=%s --config %s ; ignore_errors=no ; }\n' "$host" "$host" "$etc/host.json" > "$state/exec-kiln-host.service"
  printf '{ path=%s ; argv[]=%s --listen 100.64.5.6:8443 --host-socket /run/kiln/host.sock --token-file %s --tls-cert %s --tls-key %s ; ignore_errors=no ; }\n' \
    "$api" "$api" "$etc/admin.token" "$etc/tls.crt" "$etc/tls.key" > "$state/exec-kiln-api.service"
  printf 'active\n' > "$state/active-kiln-host.service"
  printf 'active\n' > "$state/active-kiln-api.service"
}

write_bin() {
  local name reported dest text
  for name in kiln kiln-runtime kiln-network kiln-host kiln-api; do
    if [ "$name" = kiln-host ] || [ "$name" = kiln-api ]; then reported=kiln-server; else reported=$name; fi
    if [ "$name" = kiln ] || [ "$name" = kiln-runtime ] || [ "$name" = kiln-network ]; then
      dest=$opt/bin/$name
    else
      dest=$libexec/$name
    fi
    if [ "$name" = kiln-network ]; then
      text='#!/bin/sh
exit 0
'
    elif [ "$name" = kiln-api ]; then
      # The stub is quoted so the installer does not expand it while writing the file.
      # shellcheck disable=SC2016
      text='#!/bin/sh
if [ "$1" = publish-login-token ]; then
  printf "%s\n" publish >> "$KILN_INSTALL_FIXTURE/stub-state/publish-calls"
  case "$KILN_LOGIN_MODE" in
    create)
      printf "%s" "fixture-not-a-credential" > "$KILN_INSTALL_FIXTURE/etc/kiln/login.token"
      chmod 640 "$KILN_INSTALL_FIXTURE/etc/kiln/login.token"
      printf "created\n"
      exit 0
      ;;
    fail)
      printf "login-token: unauthorized\n" >&2
      exit 1
      ;;
    *)
      printf "unchanged\n"
      exit 0
      ;;
  esac
fi
echo "'"$reported"' 0.4.4"
'
    else
      text="#!/bin/sh
echo '$reported 0.4.4'
"
    fi
    printf '%s' "$text" > "$dest"
    chmod 755 "$dest"
    cp "$dest" "$package/bin/$name"
  done
  printf '{"version":"v0.4.4","target":"x86_64-unknown-linux-gnu","min_upgrade_from":"0.3.0"}\n' > "$package/release.json"
}

run_install() {
  local n=$1 i
  shift
  : > "$work/curl.seq"
  i=0
  while [ "$i" -lt "$n" ]; do
    printf 'ok\n' >> "$work/curl.seq"
    i=$((i + 1))
  done
  set +e
  KILN_INSTALL_FIXTURE=$work KILN_LOGIN_MODE=${KILN_LOGIN_MODE:-unchanged} PATH="$work/stubs:$PATH" \
    bash "$package/install.sh" "$@" >"$work/out" 2>"$work/err"
  rc=$?
  set -e
}

marker() {
  local path=$1
  stat -c '%u %a %Y %s' "$path"
  sha256sum "$path"
}

cleanup() { rm -rf "${work:-}"; }
trap cleanup EXIT

prepare
write_bin
: > "$state/publish-calls"
printf 'active\n' > "$state/active-kiln-app.service"
printf '4100\n' > "$state/pid-kiln-app.service"
KILN_LOGIN_MODE=create run_install 1 --apply
[ "$rc" = 0 ] || fail "missing file should publish: $(cat "$work/err")"
[ -f "$etc/login.token" ] || fail "missing file was not created"
[ "$(cat "$etc/login.token")" = "fixture-not-a-credential" ] || fail "created file bytes differ"
[ "$(pid_of kiln-app.service)" != 4100 ] || fail "active kiln-app did not restart"
grep -q 'systemctl restart kiln-app.service' "$log" || fail "missing kiln-app restart"
if grep -q 'systemctl restart kiln-identity.service' "$log"; then fail "restarted kiln-identity"; fi
if grep -q 'fixture-not-a-credential' "$work/out" "$work/err" "$log"; then fail "token text was printed"; fi

prepare
write_bin
: > "$state/publish-calls"
printf 'already-present' > "$etc/login.token"
chmod 600 "$etc/login.token"
before=$(marker "$etc/login.token")
KILN_LOGIN_MODE=unchanged run_install 1 --apply
[ "$rc" = 0 ] || fail "present file should stay: $(cat "$work/err")"
after=$(marker "$etc/login.token")
[ "$before" = "$after" ] || fail "present file changed"
[ "$(wc -l < "$state/publish-calls")" -eq 1 ] || fail "present file was not offered to the publisher"
if grep -q 'systemctl restart kiln-app.service' "$log"; then fail "present file restarted kiln-app"; fi

prepare
write_bin
KILN_LOGIN_MODE=fail run_install 1 --apply
[ "$rc" != 0 ] || fail "failed login call should exit non-zero"
if [ -e "$etc/login.token" ] || [ -L "$etc/login.token" ]; then
  fail "failed login call left a file"
fi
grep -q 'already current' "$work/out" || fail "failure aborted before the rest of the upgrade: $(cat "$work/out") $(cat "$work/err")"
if grep -q 'fixture-not-a-credential' "$work/out" "$work/err" "$log"; then fail "token text was printed"; fi

prepare
write_bin
printf 'inactive\n' > "$state/active-kiln-app.service"
printf '4100\n' > "$state/pid-kiln-app.service"
KILN_LOGIN_MODE=create run_install 1 --apply
[ "$rc" = 0 ] || fail "inactive kiln-app should stay down: $(cat "$work/err")"
[ "$(pid_of kiln-app.service)" = 4100 ] || fail "inactive kiln-app was restarted"
if grep -q 'systemctl restart kiln-app.service' "$log"; then fail "restarted an inactive kiln-app"; fi
