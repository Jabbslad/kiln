#!/bin/bash
# QA swarm v0. Bash only. One ControlMaster to root@dark-forge.dev.
# Usage: scripts/qa/swarm.sh --tag <vX.Y.Z> --baseline <file> --out <dir> [--bench]
set -uo pipefail

HOST=dark-forge.dev
SSH_USER=root
BOX_ID=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
KEY_FILE=/tmp/kd
KNOWN=/tmp/qa-swarm-known_hosts
CONTROL=/tmp/qa-swarm-cm
EXPECTED_FP='SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4'
EXPECTED_HOST_FP='SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s'

tag=
baseline=
out=
do_bench=0
version=

CADDYFILE_SHA256=
CADDYFILE_INODE=
KILN_CADDY_SHA256=
CADDY_STARTED_AT=
CADDY_RESTART_COUNT=
IDENTITY_PID=
IDENTITY_NRESTARTS=

check_names=()
check_status=()
check_evidence=()

usage() {
  printf 'usage: %s --tag <vX.Y.Z> --baseline <file> --out <dir> [--bench]\n' "${0##*/}" >&2
  exit 2
}

json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/ }
  s=${s//$'\r'/}
  s=${s//$'\t'/ }
  printf '%s' "$s"
}

record() {
  local name=$1 status=$2 evidence=$3
  printf '%s %s: %s\n' "$status" "$name" "$evidence"
  check_names+=("$name")
  check_status+=("$status")
  check_evidence+=("$evidence")
}

ssh_opts() {
  SSH_OPTS=(
    -i "$KEY_FILE"
    -o IdentitiesOnly=yes
    -o StrictHostKeyChecking=yes
    -o UserKnownHostsFile="$KNOWN"
    -o ControlPath="$CONTROL"
    -o BatchMode=yes
    -o ConnectTimeout=20
  )
}

cleanup() {
  local rc=$?
  ssh_opts
  ssh "${SSH_OPTS[@]}" -o ControlMaster=no -O exit "${SSH_USER}@${HOST}" >/dev/null 2>&1 || true
  shred -u "$KEY_FILE" >/dev/null 2>&1 || true
  rm -f "$KNOWN" "$CONTROL"
  exit "$rc"
}
trap cleanup EXIT

write_key() {
  local raw=${KILN_DEPLOY_KEY-}
  local begin='-----BEGIN OPENSSH PRIVATE KEY-----'
  local end='-----END OPENSSH PRIVATE KEY-----'
  local body old
  case $raw in
    *"$begin"*"$end"*) ;;
    *)
      printf 'FAIL setup: deploy key is missing or has no OpenSSH armor\n' >&2
      return 1
      ;;
  esac
  body=${raw#*"$begin"}
  body=${body%"$end"*}
  body=$(printf '%s' "$body" | tr -d '[:space:]')
  old=$(umask)
  umask 077
  {
    printf '%s\n' "$begin"
    printf '%s\n' "$body" | fold -w 70
    printf '%s\n' "$end"
  } >"$KEY_FILE"
  chmod 600 "$KEY_FILE"
  umask "$old"
  local fp
  fp=$(ssh-keygen -lf "$KEY_FILE" 2>/dev/null || true)
  case $fp in
    *"$EXPECTED_FP"*) ;;
    *)
      printf 'FAIL setup: deploy key fingerprint did not match the pinned key\n' >&2
      return 1
      ;;
  esac
}

pin_host() {
  local scanned fp
  scanned=$(ssh-keyscan -T 10 -t ed25519 "$HOST" 2>/dev/null | awk '$2 == "ssh-ed25519" || $3 == "ssh-ed25519"')
  if [ -z "$scanned" ]; then
    printf 'FAIL setup: ssh-keyscan returned no ed25519 host key\n' >&2
    return 1
  fi
  umask 077
  printf '%s\n' "$scanned" >"$KNOWN"
  chmod 600 "$KNOWN"
  fp=$(ssh-keygen -lf "$KNOWN" 2>/dev/null || true)
  case $fp in
    *"$EXPECTED_HOST_FP"*) ;;
    *)
      printf 'FAIL setup: host key fingerprint did not match the pin\n' >&2
      return 1
      ;;
  esac
}

open_master() {
  ssh_opts
  ssh "${SSH_OPTS[@]}" -o ControlMaster=yes -o ControlPersist=900 -fN "${SSH_USER}@${HOST}"
}

remote() {
  ssh_opts
  ssh "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" "PATH=/opt/kiln/bin:\$PATH bash -s" 2>&1
}

http_code() {
  local url=$1
  shift
  local code rc
  code=$(curl --silent --show-error --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 20 --max-redirs 0 \
    --output /dev/null --write-out '%{http_code}' "$@" "$url") || rc=$?
  rc=${rc:-0}
  if [ "$rc" -ne 0 ]; then
    printf 'curl_rc=%s code=%s' "$rc" "${code:-000}"
    return 1
  fi
  printf '%s' "$code"
}

load_baseline() {
  local line key val
  [ -f "$baseline" ] || {
    printf 'FAIL setup: baseline file is missing\n' >&2
    return 1
  }
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    case $line in
      '' | '#'*) continue ;;
    esac
    key=${line%%=*}
    val=${line#*=}
    case $key in
      CADDYFILE_SHA256 | CADDYFILE_INODE | KILN_CADDY_SHA256 | CADDY_STARTED_AT | CADDY_RESTART_COUNT | IDENTITY_PID | IDENTITY_NRESTARTS)
        printf -v "$key" '%s' "$val"
        ;;
      *)
        printf 'FAIL setup: unknown baseline key %s\n' "$key" >&2
        return 1
        ;;
    esac
  done <"$baseline"
  local required name
  required='CADDYFILE_SHA256 CADDYFILE_INODE KILN_CADDY_SHA256 CADDY_STARTED_AT CADDY_RESTART_COUNT IDENTITY_PID IDENTITY_NRESTARTS'
  for name in $required; do
    if [ -z "${!name}" ]; then
      printf 'FAIL setup: baseline %s is empty\n' "$name" >&2
      return 1
    fi
  done
}

check_public() {
  local login device door term evidence
  login=$(http_code "https://${HOST}/login") || true
  device=$(http_code "https://${HOST}/device") || true
  door=$(http_code "https://cli.${HOST}/v1/boxes/${BOX_ID}/ssh" \
    --http1.1 -H 'Upgrade: kiln-ssh' -H 'Connection: Upgrade') || true
  term=$(http_code "https://app.${HOST}/v1/boxes/${BOX_ID}/terminal") || true
  evidence="login=${login} device=${device} door_ssh=${door} terminal=${term}"
  if [ "$login" = 200 ] && [ "$device" = 200 ] && [ "$door" = 401 ] && [ "$term" = 401 ]; then
    record public PASS "$evidence"
  else
    record public FAIL "$evidence"
  fi
}

check_audit() {
  local out door_n term_n door_line term_line evidence
  out=$(remote <<'REMOTE' || true
set +e
door=$(journalctl -u kiln-door.service --since "2 minutes ago" --no-pager | grep "audit ssh outcome=unauthorized" || true)
term=$(journalctl -u kiln-app.service --since "2 minutes ago" --no-pager | grep "audit terminal outcome=unauthorized" || true)
printf 'DOOR_N=%s\n' "$(printf '%s\n' "$door" | grep -c . || true)"
printf 'TERM_N=%s\n' "$(printf '%s\n' "$term" | grep -c . || true)"
printf 'DOOR_LINE=%s\n' "$(printf '%s\n' "$door" | awk 'NF { line=$0 } END { print line }')"
printf 'TERM_LINE=%s\n' "$(printf '%s\n' "$term" | awk 'NF { line=$0 } END { print line }')"
REMOTE
)
  door_n=$(printf '%s\n' "$out" | sed -n 's/^DOOR_N=//p' | head -n 1)
  term_n=$(printf '%s\n' "$out" | sed -n 's/^TERM_N=//p' | head -n 1)
  door_line=$(printf '%s\n' "$out" | sed -n 's/^DOOR_LINE=//p' | head -n 1)
  term_line=$(printf '%s\n' "$out" | sed -n 's/^TERM_LINE=//p' | head -n 1)
  door_n=${door_n:-0}
  term_n=${term_n:-0}
  evidence="door_matches=${door_n} door=${door_line} terminal_matches=${term_n} terminal=${term_line}"
  if [ "$door_n" -ge 1 ] && [ "$term_n" -ge 1 ]; then
    record audit PASS "$evidence"
  else
    record audit FAIL "$evidence"
  fi
}

check_baseline() {
  local out sha inode kiln_sha started restarts started_after restarts_after vrc vline pid nrestarts
  local evidence status login
  out=$(remote <<'REMOTE' || true
set +e
caddy=/opt/paseo/Caddyfile
kiln=/var/lib/docker/volumes/paseo_caddy_config/_data/kiln/kiln.caddy
vout=$(mktemp)
verr=$(mktemp)
printf 'CADDYFILE_SHA=%s\n' "$(sha256sum "$caddy" | awk '{ print $1 }')"
printf 'CADDYFILE_INODE=%s\n' "$(stat -c %i "$caddy")"
printf 'KILN_CADDY_SHA=%s\n' "$(sha256sum "$kiln" | awk '{ print $1 }')"
printf 'STARTED=%s\n' "$(docker inspect --format '{{.State.StartedAt}}' caddy)"
printf 'RESTARTS=%s\n' "$(docker inspect --format '{{.RestartCount}}' caddy)"
timeout 30 docker exec caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >"$vout" 2>"$verr"
printf 'VALIDATE_RC=%s\n' "$?"
printf 'VALIDATE_LINE=%s\n' "$(head -n 1 "$vout")"
printf 'STARTED_AFTER=%s\n' "$(docker inspect --format '{{.State.StartedAt}}' caddy)"
printf 'RESTARTS_AFTER=%s\n' "$(docker inspect --format '{{.RestartCount}}' caddy)"
printf 'IDENTITY_PID=%s\n' "$(systemctl show kiln-identity.service -p MainPID --value | tr -d '[:space:]')"
printf 'IDENTITY_NRESTARTS=%s\n' "$(systemctl show kiln-identity.service -p NRestarts --value | tr -d '[:space:]')"
rm -f "$vout" "$verr"
REMOTE
)
  sha=$(printf '%s\n' "$out" | sed -n 's/^CADDYFILE_SHA=//p' | head -n 1)
  inode=$(printf '%s\n' "$out" | sed -n 's/^CADDYFILE_INODE=//p' | head -n 1)
  kiln_sha=$(printf '%s\n' "$out" | sed -n 's/^KILN_CADDY_SHA=//p' | head -n 1)
  started=$(printf '%s\n' "$out" | sed -n 's/^STARTED=//p' | head -n 1)
  restarts=$(printf '%s\n' "$out" | sed -n 's/^RESTARTS=//p' | head -n 1)
  started_after=$(printf '%s\n' "$out" | sed -n 's/^STARTED_AFTER=//p' | head -n 1)
  restarts_after=$(printf '%s\n' "$out" | sed -n 's/^RESTARTS_AFTER=//p' | head -n 1)
  vrc=$(printf '%s\n' "$out" | sed -n 's/^VALIDATE_RC=//p' | head -n 1)
  vline=$(printf '%s\n' "$out" | sed -n 's/^VALIDATE_LINE=//p' | head -n 1)
  pid=$(printf '%s\n' "$out" | sed -n 's/^IDENTITY_PID=//p' | head -n 1)
  nrestarts=$(printf '%s\n' "$out" | sed -n 's/^IDENTITY_NRESTARTS=//p' | head -n 1)
  evidence="caddy_sha=${sha} inode=${inode} kiln_caddy_sha=${kiln_sha} started=${started} restarts=${restarts} validate_rc=${vrc} validate=${vline} started_after=${started_after} restarts_after=${restarts_after} identity_pid=${pid} nrestarts=${nrestarts}"
  status=PASS
  case $started in
    "$CADDY_STARTED_AT"*) ;;
    *) status=FAIL ;;
  esac
  case $started_after in
    "$CADDY_STARTED_AT"*) ;;
    *) status=FAIL ;;
  esac
  if [ "$sha" != "$CADDYFILE_SHA256" ] || [ "$inode" != "$CADDYFILE_INODE" ] || [ "$kiln_sha" != "$KILN_CADDY_SHA256" ]; then
    status=FAIL
  fi
  if [ "$restarts" != "$CADDY_RESTART_COUNT" ] || [ "$restarts_after" != "$CADDY_RESTART_COUNT" ] || [ "$vrc" != 0 ]; then
    status=FAIL
  fi
  if [ "$pid" != "$IDENTITY_PID" ]; then
    login=$(http_code "https://${HOST}/login") || true
    evidence="${evidence} login_after_pid_change=${login}"
    if [ "$status" = PASS ] && [ "$login" = 200 ]; then
      status=WARN
      evidence="${evidence} nrestarts_reported=${nrestarts}"
    else
      status=FAIL
    fi
  elif [ "$nrestarts" != "$IDENTITY_NRESTARTS" ]; then
    status=FAIL
  fi
  record baseline "$status" "$evidence"
}

check_versions() {
  local out line path reported bad skipped failed_units inactive unit_text unit active socket socket_active evidence status
  out=$(remote <<'REMOTE' || true
set +e
find /opt/kiln/bin /usr/local/libexec -maxdepth 1 -type f -name 'kiln*' -executable 2>/dev/null | sort | while read -r path; do
  base=$(basename "$path")
  case $base in
    *rollback*) printf 'SKIP %s rollback\n' "$path"; continue ;;
  esac
  if [ "$(head -c 2 "$path")" = '#!' ]; then
    printf 'SKIP %s script\n' "$path"
    continue
  fi
  line=$("$path" --version 2>/dev/null | head -n 1 || true)
  printf 'BIN %s %s\n' "$path" "$line"
done
echo '---UNITS---'
systemctl list-units --all 'kiln-*' --no-legend --no-pager
echo '---FAILED---'
systemctl list-units --failed 'kiln-*' --no-legend --no-pager
REMOTE
)
  bad=
  skipped=
  while IFS= read -r line; do
    case $line in
      '---UNITS---') break ;;
      'SKIP '*)
        skipped="${skipped} ${line#SKIP }"
        ;;
      'BIN '*)
        path=${line#BIN }
        reported=${path#* }
        path=${path%% *}
        case $reported in
          *" $version") ;;
          *) bad="${bad} ${path}=${reported}" ;;
        esac
        ;;
    esac
  done <<<"$out"
  unit_text=$(printf '%s\n' "$out" | sed -n '/^---UNITS---$/,/^---FAILED---$/p' | sed '1d;$d')
  failed_units=$(printf '%s\n' "$out" | sed -n '/^---FAILED---$/,$p' | sed '1d' | awk '$1 ~ /^kiln-/ { print $1 }' | paste -sd, -)
  failed_units=${failed_units:-none}
  inactive=
  while read -r unit _ active _; do
    case $unit in
      kiln-*) ;;
      *) continue ;;
    esac
    case $unit in
      kiln-*-proxy.service)
        socket=${unit%.service}.socket
        socket_active=$(printf '%s\n' "$unit_text" | awk -v s="$socket" '$1 == s { print $3; exit } $2 == s { print $4; exit }')
        if [ "$active" = failed ] || [ "$socket_active" != active ]; then
          inactive="${inactive} ${unit}=${active}/${socket}=${socket_active:-missing}"
        fi
        ;;
      *)
        if [ "$active" != active ]; then
          inactive="${inactive} ${unit}=${active}"
        fi
        ;;
    esac
  done <<<"$unit_text"
  inactive=${inactive:- none}
  bad=${bad:- none}
  skipped=${skipped:- none}
  evidence="version=${version} mismatch=${bad} skipped=${skipped} failed=${failed_units} not_active=${inactive}"
  if [ "$bad" = " none" ] && [ "$failed_units" = none ] && [ "$inactive" = " none" ] && printf '%s\n' "$out" | grep -q '^BIN '; then
    status=PASS
  else
    status=FAIL
  fi
  record versions "$status" "$evidence"
}

check_doctor() {
  local out rc ready evidence
  out=$(remote <<'REMOTE' || true
set +e
kiln-runtime doctor
printf '\nDOCTOR_RC=%s\n' "$?"
REMOTE
)
  rc=$(printf '%s\n' "$out" | sed -n 's/^DOCTOR_RC=//p' | head -n 1)
  if printf '%s\n' "$out" | grep -Eq '"development_ready"[[:space:]]*:[[:space:]]*true'; then
    ready=true
  else
    ready=false
  fi
  evidence="development_ready=${ready} exit=${rc:-missing}"
  if [ "$ready" = true ] && [ "$rc" = 0 ]; then
    record doctor PASS "$evidence"
  else
    record doctor FAIL "$evidence"
  fi
}

parse_boxes() {
  local tsv=$1
  local line id='' name='' pid='' state='' val
  : >"$tsv"
  while IFS= read -r line; do
    case $line in
      *'"id": "'*)
        val=${line#*\"id\": \"}
        val=${val%%\"*}
        if [[ $val =~ ^[0-9a-f]{32}$ ]]; then
          if [ -n "$id" ]; then
            printf '%s\t%s\t%s\t%s\n' "$name" "$id" "$state" "$pid" >>"$tsv"
          fi
          id=$val
          name=
          pid=
          state=
        fi
        ;;
      *'"name": "'*)
        val=${line#*\"name\": \"}
        val=${val%%\"*}
        name=$val
        ;;
      *'"pid": '*)
        val=${line#*\"pid\": }
        val=${val%%,*}
        val=${val// /}
        pid=$val
        ;;
      *'"state": "'*)
        val=${line#*\"state\": \"}
        val=${val%%\"*}
        state=$val
        ;;
    esac
  done
  if [ -n "$id" ]; then
    printf '%s\t%s\t%s\t%s\n' "$name" "$id" "$state" "$pid" >>"$tsv"
  fi
}

check_boxes() {
  local box_tsv="${out}/boxes.tsv"
  local out jamie=0 jamie2=0 extra='' bench='' qa='' line name id state pid evidence
  out=$(remote <<'REMOTE' || true
set +e
runtime=$(sed -n 's/.*"runtime_dir"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' /etc/kiln/host.json | head -n 1)
printf 'RUNTIME=%s\n' "$runtime"
if [ -n "$runtime" ]; then
  kiln-runtime --state-dir "$runtime" list
  printf '\nLIST_RC=%s\n' "$?"
else
  printf '\nLIST_RC=missing-runtime\n'
fi
if kill -0 1195266 2>/dev/null; then printf 'PID1195266=alive\n'; else printf 'PID1195266=dead\n'; fi
if kill -0 1197552 2>/dev/null; then printf 'PID1197552=alive\n'; else printf 'PID1197552=dead\n'; fi
REMOTE
)
  local list_rc p1 p2 json
  list_rc=$(printf '%s\n' "$out" | sed -n 's/^LIST_RC=//p' | head -n 1)
  p1=$(printf '%s\n' "$out" | sed -n 's/^PID1195266=//p' | head -n 1)
  p2=$(printf '%s\n' "$out" | sed -n 's/^PID1197552=//p' | head -n 1)
  json=$(printf '%s\n' "$out" | sed '/^RUNTIME=/d;/^LIST_RC=/d;/^PID1195266=/d;/^PID1197552=/d')
  parse_boxes "$box_tsv" <<<"$json"
  extra=
  bench=
  qa=
  while IFS=$'\t' read -r name id state pid; do
    [ -n "$name$id" ] || continue
    case $name in
      bench-*) bench="${bench} ${name}" ;;
      qa-*) qa="${qa} ${name}" ;;
    esac
    if [ "$name" = jamie ] && [ "$id" = 5ea2bb977e4148b98407144e2b41ad2f ] && [ "$pid" = 1195266 ] && [ "$state" = running ]; then
      jamie=1
    elif [ "$name" = jamie2 ] && [ "$id" = a340afcfc92247f8ab3f603b1ded8110 ] && [ "$pid" = 1197552 ] && [ "$state" = running ]; then
      jamie2=1
    else
      case $name in
        bench-* | qa-*) ;;
        *) extra="${extra} ${name}:${id}:${pid}:${state}" ;;
      esac
    fi
  done <"$box_tsv"
  bench=${bench:- none}
  qa=${qa:- none}
  extra=${extra:- none}
  evidence="jamie=${jamie} jamie2=${jamie2} pid1195266=${p1} pid1197552=${p2} list_rc=${list_rc} bench=${bench} qa=${qa} other=${extra}"
  if [ "$jamie" = 1 ] && [ "$jamie2" = 1 ] && [ "$p1" = alive ] && [ "$p2" = alive ] && [ "$list_rc" = 0 ] && [ "$bench" = " none" ] && [ "$qa" = " none" ]; then
    record boxes PASS "$evidence"
  else
    record boxes FAIL "$evidence"
  fi
}

find_repo() {
  local d=$PWD
  while [ "$d" != / ]; do
    if [ -e "$d/.git" ]; then
      printf '%s\n' "$d"
      return 0
    fi
    d=$(dirname "$d")
  done
  if [ -e /workspace/.git ]; then
    printf '%s\n' /workspace
    return 0
  fi
  return 1
}

check_bench() {
  local repo script_local sha remote_sha rc csv summary evidence path base
  repo=$(find_repo) || repo=
  if [ -z "$repo" ]; then
    record bench FAIL "no git checkout to read ${tag}:scripts/bench-box-start.sh"
    return
  fi
  if ! git -C "$repo" cat-file -e "${tag}:scripts/bench-box-start.sh" 2>/dev/null; then
    record bench FAIL "tag ${tag} has no scripts/bench-box-start.sh in ${repo}"
    return
  fi
  script_local=$(mktemp)
  git -C "$repo" show "${tag}:scripts/bench-box-start.sh" >"$script_local"
  sha=$(sha256sum "$script_local" | awk '{ print $1 }')
  remote <<'REMOTE' || true
set -e
rm -rf /tmp/qa-swarm-bench
mkdir -p /tmp/qa-swarm-bench/scripts
chmod 700 /tmp/qa-swarm-bench /tmp/qa-swarm-bench/scripts
REMOTE
  ssh_opts
  ssh "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" 'cat > /tmp/qa-swarm-bench/scripts/bench-box-start.sh && chmod 700 /tmp/qa-swarm-bench/scripts/bench-box-start.sh' <"$script_local"
  remote_sha=$(remote <<'REMOTE'
sha256sum /tmp/qa-swarm-bench/scripts/bench-box-start.sh | awk '{ print $1 }'
REMOTE
)
  remote_sha=$(printf '%s' "$remote_sha" | tr -d '[:space:]')
  rm -f "$script_local"
  if [ "$remote_sha" != "$sha" ]; then
    record bench FAIL "sha256 mismatch local=${sha} remote=${remote_sha}"
    return
  fi
  ssh_opts
  ssh -n "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" \
    'cd /tmp/qa-swarm-bench && bash scripts/bench-box-start.sh --mode host-socket --iterations 10 > stdout.txt 2> stderr.txt; printf "BENCH_RC=%s\n" "$?"' \
    >"$out/bench-remote-status.txt"
  rc=$(sed -n 's/^BENCH_RC=//p' "$out/bench-remote-status.txt" | head -n 1)
  ssh -n "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" 'cat /tmp/qa-swarm-bench/stdout.txt' >"$out/bench.stdout"
  ssh -n "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" 'cat /tmp/qa-swarm-bench/stderr.txt' >"$out/bench.stderr"
  ssh -n "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" \
    'find /tmp/qa-swarm-bench -type f \( -name "*.csv" -o -name "*summary*" \) ! -name stdout.txt ! -name stderr.txt -print' \
    >"$out/bench-files.txt"
  csv=''
  summary=''
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    base=${path##*/}
    ssh -n "${SSH_OPTS[@]}" -o ControlMaster=no "${SSH_USER}@${HOST}" "cat $(printf '%q' "$path")" >"$out/$base"
    case $base in
      *.csv) csv=$base ;;
      *summary*) summary=$base ;;
    esac
  done <"$out/bench-files.txt"
  if [ -z "$csv" ] && [ -s "$out/bench.stdout" ] && head -n 1 "$out/bench.stdout" | grep -q ','; then
    cp "$out/bench.stdout" "$out/bench.csv"
    csv=bench.csv
  fi
  if [ -z "$summary" ]; then
    {
      printf 'exit=%s\n' "${rc:-missing}"
      printf 'sha256=%s\n' "$sha"
    } >"$out/bench.summary.txt"
    summary='bench.summary.txt'
  fi
  evidence="sha256=${sha} exit=${rc:-missing} csv=${csv:-missing} summary=${summary}"
  if [ "$rc" = 0 ] && [ -n "$csv" ]; then
    record bench PASS "$evidence"
  else
    record bench FAIL "$evidence"
  fi
}

write_summary() {
  local overall=PASS i comma n
  n=${#check_names[@]}
  for i in "${check_status[@]}"; do
    if [ "$i" = FAIL ]; then
      overall=FAIL
    fi
  done
  mkdir -p "$out"
  {
    printf '{\n'
    printf '  "tag": "%s",\n' "$(json_escape "$tag")"
    printf '  "version": "%s",\n' "$(json_escape "$version")"
    printf '  "overall": "%s",\n' "$overall"
    printf '  "checks": [\n'
    for i in "${!check_names[@]}"; do
      comma=','
      if [ "$i" -eq $((n - 1)) ]; then
        comma=''
      fi
      printf '    {"name": "%s", "status": "%s", "evidence": "%s"}%s\n' \
        "$(json_escape "${check_names[$i]}")" \
        "$(json_escape "${check_status[$i]}")" \
        "$(json_escape "${check_evidence[$i]}")" \
        "$comma"
    done
    printf '  ]\n}\n'
  } >"$out/summary.json"
  printf 'overall %s\n' "$overall"
  [ "$overall" = PASS ]
}

while [ $# -gt 0 ]; do
  case $1 in
    --help | -h)
      printf 'usage: %s --tag <vX.Y.Z> --baseline <file> --out <dir> [--bench]\n' "${0##*/}"
      exit 0
      ;;
    --tag)
      tag=${2-}
      shift 2
      ;;
    --baseline)
      baseline=${2-}
      shift 2
      ;;
    --out)
      out=${2-}
      shift 2
      ;;
    --bench)
      do_bench=1
      shift
      ;;
    *)
      usage
      ;;
  esac
done

if [ -z "$tag" ] || [ -z "$baseline" ] || [ -z "$out" ]; then
  usage
fi
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage
version=${tag#v}
mkdir -p "$out"

if ! load_baseline || ! write_key || ! pin_host || ! open_master; then
  record setup FAIL "baseline or ssh setup failed"
  write_summary || true
  exit 1
fi

check_public
check_audit
check_baseline
check_versions
check_doctor
if [ "$do_bench" -eq 1 ]; then
  check_bench
fi
check_boxes
write_summary
