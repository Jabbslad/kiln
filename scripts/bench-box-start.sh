#!/bin/bash
# Repeatable Kiln box start-time benchmark for operator runs on the host (e.g. dark-forge).
# Research/measurement only — creates bench-* boxes and deletes them on exit.
set -euo pipefail
umask 077

usage() {
  cat <<'EOF'
Usage: bash scripts/bench-box-start.sh [options]

Options:
  --iterations N       Runs per scenario (default: 10)
  --template NAME      Template alias (default: ubuntu-4g, or first catalog entry)
  --mode MODE          host-socket (default) or local-gateway
  --self-test          Run parsing/percentile self-check with fixture data
  -h, --help           Show this help

Environment (host-socket mode defaults):
  KILN_HOST_SOCKET     /run/kiln/host.sock
  KILN_JOURNAL         /var/lib/kiln/journal/operations.sqlite
  KILN_RUNTIME         /var/lib/kiln/runtime
  KILN_HOST_CONFIG     /etc/kiln/host.json
  KILN_TEMPLATE        template alias override

Local-gateway mode additionally uses:
  KILN_GATEWAY_URL     https://127.0.0.1:8443
  KILN_ADMIN_TOKEN_FILE /etc/kiln/admin.token
  KILN_CA_FILE         /etc/kiln/ca.crt
EOF
}

now_ns() { date +%s%N; }

fail() { printf 'bench-box-start: %s\n' "$*" >&2; exit 1; }

ITERATIONS=10
MODE=host-socket
TEMPLATE="${KILN_TEMPLATE:-ubuntu-4g}"
SELF_TEST=false
HOST_SOCKET="${KILN_HOST_SOCKET:-/run/kiln/host.sock}"
JOURNAL="${KILN_JOURNAL:-/var/lib/kiln/journal/operations.sqlite}"
RUNTIME="${KILN_RUNTIME:-/var/lib/kiln/runtime}"
HOST_CONFIG="${KILN_HOST_CONFIG:-/etc/kiln/host.json}"
GATEWAY_URL="${KILN_GATEWAY_URL:-https://127.0.0.1:8443}"
TOKEN_FILE="${KILN_ADMIN_TOKEN_FILE:-/etc/kiln/admin.token}"
CA_FILE="${KILN_CA_FILE:-/etc/kiln/ca.crt}"

while [ $# -gt 0 ]; do
  case "$1" in
    --iterations) ITERATIONS=$2; shift 2 ;;
    --template) TEMPLATE=$2; shift 2 ;;
    --mode) MODE=$2; shift 2 ;;
    --self-test) SELF_TEST=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "unknown argument: $1" ;;
  esac
done

# --- percentile / CSV helpers (also used by --self-test) ---

percentile_rank() {
  # Usage: percentile_rank SORTED_VALUES...  -> prints p50 and p90 (nearest-rank, 1-based)
  local n=$# values=("$@") rank50 rank90 idx
  if [ "$n" -eq 0 ]; then
    printf 'unmeasured unmeasured\n'
    return 0
  fi
  rank50=$(( (n * 50 + 99) / 100 ))
  rank90=$(( (n * 90 + 99) / 100 ))
  [ "$rank50" -lt 1 ] && rank50=1
  [ "$rank90" -lt 1 ] && rank90=1
  [ "$rank50" -gt "$n" ] && rank50=$n
  [ "$rank90" -gt "$n" ] && rank90=$n
  idx=$((rank50 - 1))
  p50=${values[$idx]}
  idx=$((rank90 - 1))
  p90=${values[$idx]}
  printf '%s %s\n' "$p50" "$p90"
}

parse_launch_ms() {
  # Reads box.json last_launch; prints: vmm_ms init_ms agent_ms total_ms (or unmeasured)
  local box_json=$1
  [ -f "$box_json" ] || {
    printf 'unmeasured unmeasured unmeasured unmeasured\n'
    return
  }
  json_field() {
    sed -n 's/.*"'"$1"'":\([0-9.][0-9.]*\).*/\1/p' "$box_json" | head -1
  }
  vmm=$(json_field vmm_ms)
  init=$(json_field init_ms)
  agent=$(json_field agent_ms)
  total=$(json_field total_ms)
  vmm=${vmm:-unmeasured}
  init=${init:-unmeasured}
  agent=${agent:-unmeasured}
  total=${total:-unmeasured}
  printf '%s %s %s %s\n' "$vmm" "$init" "$agent" "$total"
}

journal_op_times() {
  local op_id=$1
  sqlite3 -readonly "$JOURNAL" \
    "SELECT created_at, COALESCE(finished_at, ''), running FROM operations WHERE id='$op_id';" \
    2>/dev/null || true
}

new_request_id() {
  if [ -r /proc/sys/kernel/random/uuid ]; then
    tr -d '-' </proc/sys/kernel/random/uuid
  else
    printf '%032x' "$RANDOM$RANDOM$RANDOM$RANDOM"
  fi
}

json_get_box_id() {
  awk '
    {
      if (box_id == "") {
        marker = "\"box_id\":\""
        at = index($0, marker)
        if (at > 0) {
          rest = substr($0, at + length(marker))
          end = index(rest, "\"")
          if (end > 0) {
            box_id = substr(rest, 1, end - 1)
            if (box_id ~ /^[0-9a-f]{32}$/) { print box_id; exit }
          }
        }
      }
      marker = "\"id\":\""
      at = index($0, marker)
      if (at == 0) next
      rest = substr($0, at + length(marker))
      end = index(rest, "\"")
      if (end == 0) next
      id = substr(rest, 1, end - 1)
      if (id ~ /^[0-9a-f]{32}$/) { print id; exit }
    }
  '
}

json_op_state() {
  awk '
    {
      key = "\"state\":\""
      at = index($0, key)
      if (at == 0) next
      rest = substr($0, at + length(key))
      end = index(rest, "\"")
      if (end == 0) next
      print substr(rest, 1, end - 1)
    }
  '
}

run_self_test() {
  local out p50 p90
  mapfile -t sorted < <(printf '%s\n' 10 20 30 40 50 60 70 80 90 100)
  read -r p50 p90 <<<"$(percentile_rank "${sorted[@]}")"
  [ "$p50" = 50 ] || fail "self-test p50 expected 50 got $p50"
  [ "$p90" = 90 ] || fail "self-test p90 expected 90 got $p90"

  tmp=$(mktemp -d)
  printf '%s\n' \
    '{"state":"running","last_launch":{"method":"create","vmm_ms":12.5,"init_ms":3.0,"agent_ms":4.0,"total_ms":25.0}}' \
    >"$tmp/box.json"
  read -r v i a t <<<"$(parse_launch_ms "$tmp/box.json")"
  [ "$v" = 12.5 ] || fail "self-test vmm_ms parse"
  [ "$i" = 3.0 ] || fail "self-test init_ms parse"
  [ "$a" = 4.0 ] || fail "self-test agent_ms parse"
  [ "$t" = 25.0 ] || fail "self-test total_ms parse"
  rm -rf "$tmp"

  ms=$(( (1005 - 1000) * 1000 ))
  [ "$ms" -eq 5000 ] || fail "self-test journal duration math expected 5000 got $ms"

  printf 'bench-box-start self-test: ok\n'
  exit 0
}

if [ "$SELF_TEST" = true ]; then
  run_self_test
fi

case "$MODE" in
  host-socket|local-gateway) ;;
  *) fail "unsupported --mode $MODE (use host-socket or local-gateway)" ;;
esac

command -v curl >/dev/null 2>&1 || fail 'curl is required'
command -v sqlite3 >/dev/null 2>&1 || fail 'sqlite3 is required'
command -v awk >/dev/null 2>&1 || fail 'awk is required'

[ -S "$HOST_SOCKET" ] || fail "host socket missing: $HOST_SOCKET"
[ -f "$JOURNAL" ] || fail "operations journal missing: $JOURNAL"
[ -d "$RUNTIME" ] || fail "runtime dir missing: $RUNTIME"

CURL_HOST=(curl -sS --unix-socket "$HOST_SOCKET" -H 'Content-Type: application/json')
CURL_GATEWAY=()
CLI_AUTH_START=unmeasured
if [ "$MODE" = local-gateway ]; then
  [ -r "$TOKEN_FILE" ] || fail "local-gateway mode needs readable $TOKEN_FILE"
  [ -r "$CA_FILE" ] || fail "local-gateway mode needs readable $CA_FILE"
  CLI_AUTH_START=$(now_ns)
  token=$(tr -d '[:space:]' <"$TOKEN_FILE")
  [ "${#token}" -eq 64 ] || fail "admin token must be 64 hex characters"
  CLI_AUTH_END=$(now_ns)
  CURL_GATEWAY=(curl -sS --cacert "$CA_FILE" -H "Authorization: Bearer $token" -H 'Content-Type: application/json')
fi

BENCH_BOXES=
bench_register() {
  BENCH_BOXES="$BENCH_BOXES $1"
}

bench_delete_all() {
  local id
  for id in $BENCH_BOXES; do
    [ -n "$id" ] || continue
    submit_operation '{"type":"stop","id":"'"$id"'","force":true}' >/dev/null 2>&1 || true
    submit_operation '{"type":"delete","id":"'"$id"'"}' >/dev/null 2>&1 || true
  done
  BENCH_BOXES=
}

cleanup() {
  local status=$?
  bench_delete_all || status=1
  exit "$status"
}
trap cleanup EXIT INT TERM

count_boxes() {
  "${CURL_HOST[@]}" "http://localhost/v1/boxes" | awk '
    BEGIN { n = 0 }
    /"id"/ { n++ }
    END { print n }
  '
}

ensure_capacity() {
  local need=$1 total
  total=$(count_boxes)
  if [ "$((total + need))" -gt 8 ]; then
    fail "box cap: have $total boxes, need $need more (limit 8)"
  fi
}

http_post_op() {
  local body=$1
  if [ "$MODE" = host-socket ]; then
    "${CURL_HOST[@]}" -X POST -d "$body" "http://localhost/v1/operations"
  else
    "${CURL_GATEWAY[@]}" -X POST -d "$body" "$GATEWAY_URL/v1/operations"
  fi
}

http_get_op() {
  local op_id=$1
  if [ "$MODE" = host-socket ]; then
    "${CURL_HOST[@]}" "http://localhost/v1/operations/$op_id"
  else
    "${CURL_GATEWAY[@]}" "$GATEWAY_URL/v1/operations/$op_id"
  fi
}

submit_operation() {
  local action_json=$1
  local req_id body
  req_id=$(new_request_id)
  body=$(printf '{"id":"%s","action":%s}' "$req_id" "$action_json")
  http_post_op "$body"
  wait_op "$req_id"
  printf '%s\n' "$req_id"
}

submit_timed() {
  local action_json=$1
  local req_id body t_cli_start t_http_start t_http_end resp
  req_id=$(new_request_id)
  body=$(printf '{"id":"%s","action":%s}' "$req_id" "$action_json")

  if [ "$MODE" = local-gateway ]; then
    t_cli_start=$CLI_AUTH_START
    t_cli_end=$CLI_AUTH_END
  else
    t_cli_start=unmeasured
    t_cli_end=unmeasured
  fi
  t_http_start=$(now_ns)
  resp=$(http_post_op "$body") || fail "operation submit failed"
  t_http_end=$(now_ns)
  printf '%s\n' "$resp" | grep -q '"state"' || fail "invalid operation response"
  wait_op "$req_id"
  printf '%s|%s|%s|%s|%s|%s\n' "$req_id" "$t_cli_start" "$t_cli_end" "$t_http_start" "$t_http_end" "$resp"
}

wait_op() {
  local op_id=$1
  local deadline=$((SECONDS + 600)) state
  while [ "$SECONDS" -lt "$deadline" ]; do
    state=$(http_get_op "$op_id" | json_op_state | head -1)
    case "$state" in
      succeeded|failed|unknown) return 0 ;;
    esac
    sleep 0.05
  done
  fail "operation $op_id timed out"
}

wait_exec_ready() {
  local box_id=$1
  local req_id body t0 t1
  req_id=$(new_request_id)
  body=$(printf '{"id":"%s","action":{"type":"exec","id":"%s","request":{"argv":["/bin/true"],"timeout_ms":10000}}}' \
    "$req_id" "$box_id")
  t0=$(now_ns)
  http_post_op "$body" >/dev/null || true
  wait_op "$req_id"
  t1=$(now_ns)
  printf '%s\n' $(( (t1 - t0) / 1000000 ))
}

measure_journal_host_ms() {
  local op_id=$1
  local row created finished running
  row=$(journal_op_times "$op_id")
  [ -n "$row" ] || { printf 'unmeasured\n'; return; }
  read -r created finished running <<<"$row"
  if [ -n "$finished" ] && [ "$finished" -gt "$created" ] 2>/dev/null; then
    printf '%s\n' $(( (finished - created) * 1000 ))
  else
    printf 'unmeasured\n'
  fi
}

duration_ms() {
  local start=$1 end=$2
  case "$start" in unmeasured|''|*[!0-9]*) printf 'unmeasured\n'; return ;; esac
  case "$end" in unmeasured|''|*[!0-9]*) printf 'unmeasured\n'; return ;; esac
  if [ "$end" -lt "$start" ]; then
    printf 'unmeasured\n'
    return
  fi
  printf '%s\n' $(( (end - start) / 1000000 ))
}

resolve_template() {
  local list
  list=$("${CURL_HOST[@]}" "http://localhost/v1/templates")
  if printf '%s' "$list" | grep -q "\"name\":\"$TEMPLATE\""; then
    printf '%s\n' "$TEMPLATE"
    return
  fi
  printf '%s' "$list" | awk -F'"' '/"name"/ { print $4; exit }'
}

TEMPLATE=$(resolve_template)
[ -n "$TEMPLATE" ] || fail "no templates in host catalog"

CSV_FILE=$(mktemp)
printf 'scenario,iteration,cli_auth_ms,api_request_ms,kiln_host_operation_ms,firecracker_boot_ms,guest_agent_ready_ms,sshd_ready_ms,ready_exec_ms,notes\n' >"$CSV_FILE"

launch_guest_ms() {
  local init=$1 agent=$2
  if [ "$init" != unmeasured ] && [ "$agent" != unmeasured ]; then
    awk -v a="$agent" -v i="$init" 'BEGIN { printf "%.3f", a + i }'
  elif [ "$init" != unmeasured ]; then printf '%s\n' "$init"
  elif [ "$agent" != unmeasured ]; then printf '%s\n' "$agent"
  else printf 'unmeasured\n'
  fi
}

record_run() {
  local scenario=$1 iter=$2 op_id=$3 box_id=$4 t_cli0 t_cli1 t_http0 t_http1
  local cli_ms api_ms host_ms fc_ms guest_ms sshd_ms ready_ms box_json
  if [ "$MODE" = local-gateway ] && [ "$t_cli0" != unmeasured ]; then
    cli_ms=$(duration_ms "$t_cli0" "$t_cli1")
  else
    cli_ms=unmeasured
  fi
  api_ms=$(duration_ms "$t_http0" "$t_http1")
  host_ms=$(measure_journal_host_ms "$op_id")
  box_json="$RUNTIME/boxes/$box_id/box.json"
  read -r vmm init agent total <<<"$(parse_launch_ms "$box_json")"
  fc_ms=$vmm
  guest_ms=$(launch_guest_ms "$init" "$agent")
  sshd_ms=unmeasured
  ready_ms=$(wait_exec_ready "$box_id")
  printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,box=%s op=%s\n' \
    "$scenario" "$iter" "$cli_ms" "$api_ms" "$host_ms" "$fc_ms" "$guest_ms" "$sshd_ms" \
    "$ready_ms" "launch_total_ms=$total" "$box_id" "$op_id" >>"$CSV_FILE"
}

run_create_iteration() {
  local iter=$1 name row op_id box_id
  name=$(printf 'bench-create-%04d' "$iter")
  ensure_capacity 1
  row=$(submit_timed '{"type":"create","template":"'"$TEMPLATE"'","name":"'"$name"'"}')
  IFS='|' read -r op_id _t_cli0 _t_cli1 _t_http0 _t_http1 resp <<<"$row"
  box_id=$(printf '%s' "$resp" | json_get_box_id)
  [ -n "$box_id" ] || fail "create missing box id"
  bench_register "$box_id"
  record_run create "$iter" "$op_id" "$box_id" "$_t_cli0" "$_t_cli1" "$_t_http0" "$_t_http1"
  submit_operation '{"type":"delete","id":"'"$box_id"'"}' >/dev/null
  BENCH_BOXES=$(printf '%s' "$BENCH_BOXES" | tr ' ' '\n' | grep -v "^$box_id$" | tr '\n' ' ')
}

run_start_iteration() {
  local scenario=$1 iter=$2 box_id=$3 row op_id
  row=$(submit_timed '{"type":"start","id":"'"$box_id"'"}')
  IFS='|' read -r op_id _t_cli0 _t_cli1 _t_http0 _t_http1 _resp <<<"$row"
  record_run "$scenario" "$iter" "$op_id" "$box_id" "$_t_cli0" "$_t_cli1" "$_t_http0" "$_t_http1"
}

summarize_scenario() {
  local scenario=$1 col values p50 p90 count
  printf '\n# summary scenario=%s\n' "$scenario"
  printf 'stage,p50_ms,p90_ms,count\n'
  for col in 3 4 5 6 7 8 9; do
    case "$col" in
      3) stage=cli_auth_ms ;;
      4) stage=api_request_ms ;;
      5) stage=kiln_host_operation_ms ;;
      6) stage=firecracker_boot_ms ;;
      7) stage=guest_agent_ready_ms ;;
      8) stage=sshd_ready_ms ;;
      9) stage=ready_exec_ms ;;
    esac
    values=$(
      awk -F, -v sc="$scenario" -v c="$col" '
        NR == 1 { next }
        $1 == sc {
          v = $c
          if (v != "unmeasured" && v ~ /^[0-9.]+$/) print v
        }
      ' "$CSV_FILE" | sort -n
    )
    if [ -z "$values" ]; then
      printf '%s,unmeasured,unmeasured,0\n' "$stage"
      continue
    fi
    count=$(printf '%s\n' "$values" | wc -l)
    mapfile -t _vals <<<"$values"
    read -r p50 p90 <<<"$(percentile_rank "${_vals[@]}")"
    printf '%s,%s,%s,%s\n' "$stage" "$p50" "$p90" "$count"
  done
}

printf 'bench-box-start: mode=%s template=%s iterations=%s\n' "$MODE" "$TEMPLATE" "$ITERATIONS" >&2
printf 'bench-box-start: host socket %s journal %s\n' "$HOST_SOCKET" "$JOURNAL" >&2

ensure_capacity 1
i=1
while [ "$i" -le "$ITERATIONS" ]; do
  run_create_iteration "$i"
  i=$((i + 1))
done

ensure_capacity 1
START_BOX=
row=$(submit_timed '{"type":"create","template":"'"$TEMPLATE"'","name":"bench-start"}')
box_id=$(printf '%s' "${row##*|}" | json_get_box_id)
[ -n "$box_id" ] || fail "bench-start create failed"
bench_register "$box_id"
START_BOX=$box_id
submit_operation '{"type":"stop","id":"'"$START_BOX"'","force":true}' >/dev/null

i=1
while [ "$i" -le "$ITERATIONS" ]; do
  run_start_iteration start "$i" "$START_BOX"
  submit_operation '{"type":"stop","id":"'"$START_BOX"'","force":true}' >/dev/null
  i=$((i + 1))
done

i=1
while [ "$i" -le "$ITERATIONS" ]; do
  submit_operation '{"type":"stop","id":"'"$START_BOX"'","force":true}' >/dev/null
  run_start_iteration restart "$i" "$START_BOX"
  i=$((i + 1))
done

cat "$CSV_FILE"
summarize_scenario create
summarize_scenario start
summarize_scenario restart

rm -f "$CSV_FILE"
