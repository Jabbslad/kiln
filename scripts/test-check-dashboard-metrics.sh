#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-check-dashboard-metrics: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
check=$root/deploy/github/check-dashboard-metrics.sh
[ -x "$check" ] || chmod +x "$check"

dashboard_fetch=$work/fetch.sh
cat >"$dashboard_fetch" <<EOF
#!/bin/sh
cat "\$1"
EOF
chmod +x "$dashboard_fetch"

host_only='{"host":{"metrics":{"cpu_percent":1.5,"memory_bytes":1000,"disk_bytes":null}},"boxes":[]}'
PATH="$work:$PATH"
export KILN_ADMIN_TOKEN_FILE="$work/token"
export KILN_CA_FILE="$work/ca"
printf '%064d' 0 >"$KILN_ADMIN_TOKEN_FILE"
printf 'cert\n' >"$KILN_CA_FILE"

cat >"$work/curl" <<EOF
#!/bin/sh
case "\$*" in
  *dashboard*) cat "$work/body" ;;
  *) exec /usr/bin/curl "\$@" ;;
esac
EOF
chmod +x "$work/curl"

printf '%s' "$host_only" >"$work/body"
PATH="$work:$PATH" bash "$check" 127.0.0.1 >/dev/null \
  || fail 'host-only dashboard should pass'

printf '{"host":{"metrics":{"cpu_percent":null,"memory_bytes":1000}},"boxes":[]}' >"$work/body"
if PATH="$work:$PATH" bash "$check" 127.0.0.1 >/dev/null 2>&1; then
  fail 'null host cpu must fail'
fi

running='{"host":{"metrics":{"cpu_percent":2,"memory_bytes":2000}},"boxes":[{"state":"running","metrics":{"cpu_percent":null,"memory_bytes":100}}]}'
printf '%s' "$running" >"$work/body"
if PATH="$work:$PATH" bash "$check" 127.0.0.1 >/dev/null 2>&1; then
  fail 'first fetch with running box and null metrics must fail before second fetch'
fi

running_ok='{"host":{"metrics":{"cpu_percent":2,"memory_bytes":2000}},"boxes":[{"state":"running","metrics":{"cpu_percent":3.5,"memory_bytes":100}}]}'
printf '%s' "$running_ok" >"$work/body"
PATH="$work:$PATH" bash "$check" 127.0.0.1 >/dev/null \
  || fail 'running box with metrics should pass on second fetch'

printf 'check-dashboard-metrics tests ok\n'
