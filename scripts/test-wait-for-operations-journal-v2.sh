#!/bin/bash
# wait_for_operations must accept a real sqlite journal at user_version=2 (schema v2).
# The pre-fix installer required user_version=1 and blocked server binary upgrades.
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
install=$root/deploy/install.sh
fail() { printf 'wait-for-operations-journal-v2: %s\n' "$*" >&2; exit 1; }

command -v sqlite3 >/dev/null 2>&1 || fail 'sqlite3 is required'

prepare() {
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
    if [ -f "$state/pid-\$service" ]; then old=\$(tr -d '[:space:]' < "$state/pid-\$service"); fi
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
  cp "$root/deploy/operations-journal.max-version" "$package/operations-journal.max-version"
  chmod 755 "$package/install.sh"
  printf 'identity-bytes-v1' > "$libexec/kiln-identity"
  custom=$(printf 'a%.0s' $(seq 1 32))
  printf '{"runtime_dir":"%s/runtime","journal_dir":"%s/journal","socket":"/run/kiln/host.sock","isolation_config":"%s/isolation.json","templates":{"custom":"%s"}}\n' \
    "$var" "$var" "$etc" "$custom" > "$etc/host.json"
  printf '{}\n' > "$etc/isolation.json"
  for name in ca.crt tls.crt tls.key admin.token; do printf 'synthetic-test-only\n' > "$etc/$name"; done
  host=$libexec/kiln-host
  api=$libexec/kiln-api
  printf '{ path=%s ; argv[]=%s --config %s ; ignore_errors=no ; }\n' "$host" "$host" "$etc/host.json" > "$state/exec-kiln-host.service"
  printf '{ path=%s ; argv[]=%s --listen 100.64.5.6:8443 --host-socket /run/kiln/host.sock --token-file %s --tls-cert %s --tls-key %s ; ignore_errors=no ; }\n' \
    "$api" "$api" "$etc/admin.token" "$etc/tls.crt" "$etc/tls.key" > "$state/exec-kiln-api.service"
  printf 'active\n' > "$state/active-kiln-host.service"
  printf 'active\n' > "$state/active-kiln-api.service"
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
  local installed=$1 target=$2 name reported dest text
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

run_install() {
  local n=$1
  shift
  : > "$work/curl.seq"
  i=0
  while [ "$i" -lt "$n" ]; do
    printf 'ok\n' >> "$work/curl.seq"
    i=$((i + 1))
  done
  set +e
  KILN_INSTALL_FIXTURE=$work PATH="$work/stubs:$PATH" bash "$package/install.sh" "$@" >"$work/out" 2>"$work/err"
  rc=$?
  set -e
}

cleanup() { rm -rf "${work:-}"; }
trap cleanup EXIT

prepare
printf '{"version":"v0.4.5","target":"x86_64-unknown-linux-gnu","min_upgrade_from":"0.3.0"}\n' > "$package/release.json"
write_bin 0.4.4 0.4.5
init_journal_v2
printf '4100\n' > "$state/pid-kiln-app.service"
printf '4200\n' > "$state/pid-kiln-door.service"
run_install 2 --apply
[ "$rc" = 0 ] || fail "v2 journal blocked upgrade: $(cat "$work/err")"
grep -q '0.4.4 -> 0.4.5' "$work/out" || fail "missing upgrade banner: $(cat "$work/out")"

printf 'wait for operations journal v2 ok\n'
