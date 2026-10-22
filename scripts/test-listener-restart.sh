#!/bin/bash
# Prove kiln-app and kiln-door restart when kiln-api or their units change.
# Fixture only. Does not touch a real host.
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
install=$root/deploy/install.sh
fail() { printf 'listener-restart: %s\n' "$*" >&2; exit 1; }

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
      LoadState)
        if [ -f "$state/load-state" ]; then cat "$state/load-state"; else printf '%s\\n' not-found; fi
        ;;
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
    if [ -f "$state/fail-restart-\$service" ]; then
      rm -f "$state/fail-restart-\$service"
      exit 1
    fi
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
  cp "$root/deploy/operations-journal.max-version" "$package/operations-journal.max-version"
  printf 'identity-bytes-v1' > "$libexec/kiln-identity"
  custom=$(printf 'a%.0s' $(seq 1 32))
  printf '{"runtime_dir":"%s/runtime","journal_dir":"%s/journal","socket":"/run/kiln/host.sock","isolation_config":"%s/isolation.json","templates":{"custom":"%s"}}\n' \
    "$var" "$var" "$etc" "$custom" > "$etc/host.json"
  printf '{}\n' > "$etc/isolation.json"
  for name in ca.crt tls.crt tls.key admin.token; do printf 'synthetic-test-only\n' > "$etc/$name"; done
  init_journal_v1
  host=$libexec/kiln-host
  api=$libexec/kiln-api
  printf '{ path=%s ; argv[]=%s --config %s ; ignore_errors=no ; }\n' "$host" "$host" "$etc/host.json" > "$state/exec-kiln-host.service"
  printf '{ path=%s ; argv[]=%s --listen 100.64.5.6:8443 --host-socket /run/kiln/host.sock --token-file %s --tls-cert %s --tls-key %s ; ignore_errors=no ; }\n' \
    "$api" "$api" "$etc/admin.token" "$etc/tls.crt" "$etc/tls.key" > "$state/exec-kiln-api.service"
  printf 'active\n' > "$state/active-kiln-host.service"
  printf 'active\n' > "$state/active-kiln-api.service"
}

init_journal_v1() {
  rm -f "$var/journal/operations.sqlite"
  sqlite3 "$var/journal/operations.sqlite" \
    'PRAGMA journal_mode=DELETE; PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);' \
    >/dev/null
}

init_journal_v3() {
  rm -f "$var/journal/operations.sqlite"
  sqlite3 "$var/journal/operations.sqlite" <<'SQL'
PRAGMA journal_mode=DELETE;
PRAGMA user_version=3;
CREATE TABLE operations (
  id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
  is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
  created_at INTEGER NOT NULL, finished_at INTEGER,
  action TEXT NOT NULL DEFAULT 'unknown');
INSERT INTO operations(
  id, fingerprint, box_id, is_create, running, response, created_at, finished_at, action
) VALUES (
  '0123456789abcdef0123456789abcde1', X'00', '0123456789abcdef0123456789abcdef',
  1, 0, '{"state":"succeeded"}', 1700000000, 1700000001, 'create');
SQL
}

init_journal_v2() {
  rm -f "$var/journal/operations.sqlite"
  sqlite3 "$var/journal/operations.sqlite" <<'SQL'
PRAGMA journal_mode=DELETE;
PRAGMA user_version=2;
CREATE TABLE operations (
  id TEXT PRIMARY KEY, fingerprint BLOB NOT NULL, box_id TEXT NOT NULL,
  is_create INTEGER NOT NULL, running INTEGER NOT NULL, response TEXT NOT NULL,
  created_at INTEGER NOT NULL, finished_at INTEGER,
  action TEXT NOT NULL DEFAULT 'unknown');
INSERT INTO operations(
  id, fingerprint, box_id, is_create, running, response, created_at, finished_at, action
) VALUES (
  '0123456789abcdef0123456789abcde1', X'00', '0123456789abcdef0123456789abcdef',
  1, 0, '{"state":"succeeded"}', 1700000000, 1700000001, 'create');
SQL
}

write_bin() {
  local installed=$1 target=$2
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
    else
      text="#!/bin/sh
echo '$reported $installed'
"
    fi
    printf '%s' "$text" > "$dest"
    chmod 755 "$dest"
    if [ "$name" = kiln-network ]; then
      text='#!/bin/sh
exit 0
'
    else
      text="#!/bin/sh
echo '$reported $target'
"
    fi
    printf '%s' "$text" > "$package/bin/$name"
    chmod 755 "$package/bin/$name"
  done
}

write_release() {
  printf '{"version":"v%s","target":"x86_64-unknown-linux-gnu","min_upgrade_from":"0.3.0"}\n' "$1" > "$package/release.json"
}

mark() { printf 'active\n' > "$state/active-$1"; printf '%s\n' "$2" > "$state/pid-$1"; }

run_install() {
  local n=$1 i clear_seq=1
  shift
  if [ "$n" -ge 0 ]; then
    case ${1:-} in
      --keep-curl-seq)
        clear_seq=0
        shift
        ;;
    esac
  fi
  if [ "$clear_seq" = 1 ]; then
    : > "$work/curl.seq"
    i=0
    while [ "$i" -lt "$n" ]; do
      printf 'ok\n' >> "$work/curl.seq"
      i=$((i + 1))
    done
  fi
  set +e
  KILN_INSTALL_FIXTURE=$work PATH="$work/stubs:$PATH" bash "$package/install.sh" "$@" >"$work/out" 2>"$work/err"
  rc=$?
  set -e
}

cleanup() { rm -rf "${work:-}"; }
trap cleanup EXIT

prepare
write_release 0.4.4
write_bin 0.3.2 0.4.4
mark kiln-app.service 4100
mark kiln-door.service 4200
mark kiln-identity.service 4300
run_install 2 --apply
[ "$rc" = 0 ] || fail "replaced kiln-api should install: $(cat "$work/err")"
[ "$(pid_of kiln-app.service)" != 4100 ] || fail "kiln-app kept pid 4100"
[ "$(pid_of kiln-door.service)" != 4200 ] || fail "kiln-door kept pid 4200"
[ "$(pid_of kiln-identity.service)" = 4300 ] || fail "kiln-identity pid changed"
grep -qx 'identity-bytes-v1' "$libexec/kiln-identity"
grep -q 'systemctl restart kiln-app.service' "$log"
grep -q 'systemctl restart kiln-door.service' "$log"
if grep -q 'systemctl restart kiln-identity.service' "$log"; then fail "restarted kiln-identity"; fi
if grep -q 'systemctl stop kiln-network.service' "$log"; then fail "stopped kiln-network"; fi
if grep -q 'systemctl restart kiln-network.service' "$log"; then fail "restarted kiln-network"; fi

prepare
write_release 0.4.4
write_bin 0.4.4 0.4.4
mark kiln-app.service 4100
mark kiln-door.service 4200
run_install 1 --apply
[ "$rc" = 0 ] || fail "unchanged install failed: $(cat "$work/err")"
grep -q 'already current' "$work/out" || fail "missing already current"
[ "$(pid_of kiln-app.service)" = 4100 ] || fail "unchanged kiln-app restarted"
[ "$(pid_of kiln-door.service)" = 4200 ] || fail "unchanged kiln-door restarted"
if grep -q 'systemctl restart kiln-app.service' "$log"; then fail "restarted kiln-app when the binary matched"; fi

prepare
write_release 0.4.5
write_bin 0.4.5 0.4.5
printf '# rebuilt\n' >> "$package/bin/kiln"
mark kiln-app.service 4100
mark kiln-door.service 4200
run_install 2 --apply
[ "$rc" = 0 ] || fail "other binary upgrade failed: $(cat "$work/err")"
[ "$(pid_of kiln-app.service)" = 4100 ] || fail "kiln-app restarted for a client-only change"
[ "$(pid_of kiln-door.service)" = 4200 ] || fail "kiln-door restarted for a client-only change"

prepare
write_release 0.4.4
write_bin 0.4.4 0.4.4
mkdir -p "$package/deploy" "$work/usr/local/share/kiln"
for unit in kiln-app.service kiln-app-proxy.service kiln-app-proxy.socket; do
  cp "$root/deploy/$unit" "$package/deploy/$unit"
done
printf '%s\n# stale\n' "$(cat "$root/deploy/kiln-app.service")" > "$work/etc/systemd/system/kiln-app.service"
printf '%s\n# stale\n' "$(cat "$root/deploy/kiln-app-proxy.socket")" > "$work/etc/systemd/system/kiln-app-proxy.socket"
mark kiln-app.service 4100
mark kiln-app-proxy.socket 5100
dash=$work/dashboard.tar.gz
tmpdir=$work/dashsrc
mkdir -p "$tmpdir"
printf '<html>one</html>\n' > "$tmpdir/index.html"
tar -C "$tmpdir" -czf "$dash" index.html
run_install 1 --apply --dashboard-archive "$dash"
[ "$rc" = 0 ] || fail "unit restart install failed: $(cat "$work/err") $(cat "$work/out")"
cmp -s "$work/etc/systemd/system/kiln-app.service" "$root/deploy/kiln-app.service" || fail "unit was not replaced"
[ "$(pid_of kiln-app.service)" != 4100 ] || fail "changed app unit left pid 4100"
[ "$(pid_of kiln-app-proxy.socket)" != 5100 ] || fail "changed socket left pid 5100"
if grep -q 'systemctl restart kiln-app-proxy.service' "$log"; then fail "restarted the proxy service"; fi

prepare
write_release 0.4.4
write_bin 0.3.2 0.4.4
mark kiln-app.service 4100
printf '1\n' > "$state/fail-restart-kiln-app.service"
before=$(cat "$libexec/kiln-api")
run_install 3 --apply
[ "$rc" != 0 ] || fail "restart failure should restore"
grep -q restored "$work/err" || fail "missing restored: $(cat "$work/err")"
[ "$(cat "$libexec/kiln-api")" = "$before" ] || fail "kiln-api was not restored"
[ "$(pid_of kiln-app.service)" != 4100 ] || fail "restored listener kept pid 4100"
count=$(grep -c 'systemctl restart kiln-app.service' "$log" || true)
[ "$count" -ge 2 ] || fail "expected two restarts, saw $count"

# A v2 operation journal must not block binary swap on an installed host.
prepare
write_release 0.4.5
write_bin 0.4.4 0.4.5
init_journal_v2
mark kiln-app.service 4100
mark kiln-door.service 4200
run_install 2 --apply
[ "$rc" = 0 ] || fail "v2 journal upgrade failed: $(cat "$work/err")"
grep -q '0.4.4 -> 0.4.5' "$work/out" || fail "missing upgrade banner: $(cat "$work/out")"

# Unsupported journal schema versions fail with the sqlite command and version range.
prepare
write_release 0.4.5
write_bin 0.4.4 0.4.5
init_journal_v3
mark kiln-app.service 4100
run_install 1 --apply
[ "$rc" != 0 ] || fail "unsupported journal version should fail the swap"
grep -q 'install step failed: wait for journal idle' "$work/err" \
  || fail "missing step name: $(cat "$work/err")"
grep -q "PRAGMA user_version;" "$work/err" || fail "missing journal command: $(cat "$work/err")"
grep -q 'found operation journal schema version 3' "$work/err" \
  || fail "missing found version: $(cat "$work/err")"
grep -q 'installer accepts 1 through 2' "$work/err" \
  || fail "missing expected range: $(cat "$work/err")"
grep -q 'exit code: 1' "$work/err" || fail "missing exit code: $(cat "$work/err")"

# Failed update steps print the step name, command, and exit code to stderr.
prepare
write_release 0.4.5
write_bin 0.4.4 0.4.5
init_journal_v1
mark kiln-app.service 4100
mark kiln-door.service 4200
{
  printf 'ok\n'
  i=0
  while [ "$i" -lt 35 ]; do
    printf 'fail\n'
    i=$((i + 1))
  done
  printf 'ok\n'
} > "$work/curl.seq"
run_install 0 --keep-curl-seq --apply
[ "$rc" != 0 ] || fail "healthcheck failure should fail the swap"
grep -q 'install step failed: healthcheck after binary swap' "$work/err" \
  || fail "missing step name: $(cat "$work/err")$(cat "$work/out")"
grep -q 'exit code: 1' "$work/err" || fail "missing exit code: $(cat "$work/err")"
grep -q 'update failed during binary swap; install step details are on stderr' "$work/err" \
  || fail "missing rollback message: $(cat "$work/err")"

rm -rf "$work"
trap - EXIT
printf 'listener restart checks passed\n'
