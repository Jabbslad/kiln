#!/bin/bash
# Enroll this host with https://dark-forge.dev.
# Laptops use https://cli.dark-forge.dev on port 443.
# The account must be GitHub user Jabbslad, numeric id 3084.
# The directory token is written to a mode 0600 header file. curl reads that
# file with -H @file. The token is never placed on an argv.
# An enrollment that already exists is left in place unless this run created it.
# kiln-identity.service is not restarted and its unit file is not edited.
set -euo pipefail
umask 077

public_host=cli.dark-forge.dev
identity_origin=https://dark-forge.dev
expected_origin=https://cli.dark-forge.dev
expected_login=Jabbslad
expected_subject=3084
libexec=/usr/local/libexec/kiln-api
etc=/etc/kiln
db=/var/lib/kiln-identity/identity.db
dropin=/etc/systemd/system/kiln-api.service.d/identity.conf
unit_src=$(dirname "$0")/kiln-door.service

die() {
  printf '%s\n' "$1" >&2
  exit 1
}

parse_identity() {
  owner=$(sed -n 's/.*"owner_id":"\([^"]*\)".*/\1/p' "$etc/identity.json")
  server_id=$(sed -n 's/.*"server_id":"\([^"]*\)".*/\1/p' "$etc/identity.json")
  case $owner in
    "" | *[!A-Za-z0-9_-]*) return 1 ;;
  esac
  case $server_id in
    "" | *[!A-Za-z0-9_-]*) return 1 ;;
  esac
}

account_and_origin_match() {
  local extra lines origin
  subject=$(sqlite3 -readonly "$db" \
    "SELECT subject FROM provider_identities WHERE account_id='$owner' AND provider='github' AND display_name='$expected_login' AND subject='$expected_subject';")
  extra=$(sqlite3 -readonly "$db" \
    "SELECT count(*) FROM provider_identities WHERE account_id='$owner' AND NOT (provider='github' AND display_name='$expected_login' AND subject='$expected_subject');")
  lines=$(printf '%s\n' "$subject" | grep -c . || true)
  [ "$lines" = 1 ] && [ "$subject" = "$expected_subject" ] && [ "$extra" = 0 ] || return 1
  origin=$(sqlite3 -readonly "$db" \
    "SELECT origin FROM servers WHERE id='$server_id' AND active=1;")
  lines=$(printf '%s\n' "$origin" | grep -c . || true)
  [ "$lines" = 1 ] && [ "$origin" = "$expected_origin" ]
}

# enrollment_state is absent, match, mismatch, or incomplete.
# Called in the current shell so owner, server_id, and subject stay set.
assess_enrollment() {
  local present=0 path
  for path in "$etc/identity.json" "$etc/directory.token" "$etc/enrollment.pending.json" "$dropin"; do
    if [ -e "$path" ]; then
      present=1
    fi
  done
  if [ "$present" = 0 ]; then
    enrollment_state=absent
    return 0
  fi
  if [ ! -f "$etc/identity.json" ]; then
    enrollment_state=incomplete
    return 0
  fi
  if ! parse_identity || ! account_and_origin_match; then
    enrollment_state=mismatch
    return 0
  fi
  enrollment_state=match
}

# created is 1 only for files this process's enroll command wrote.
plan_action() {
  local state=$1 created=$2
  case $state in
    absent) printf '%s\n' enroll ;;
    match) printf '%s\n' enable ;;
    mismatch | incomplete)
      if [ "$created" = 1 ]; then
        printf '%s\n' rollback
      else
        printf '%s\n' refuse
      fi
      ;;
    *) return 1 ;;
  esac
}

write_bearer_header() {
  local token_file=$1 dest=$2
  chmod 600 "$dest"
  {
    printf 'Authorization: Bearer '
    cat "$token_file"
    printf '\n'
  } >"$dest"
  chmod 600 "$dest"
}

deactivate_directory() {
  local hdr
  [ -f "$etc/directory.token" ] || return 0
  [ -n "${server_id:-}" ] || return 0
  hdr=$(mktemp)
  write_bearer_header "$etc/directory.token" "$hdr"
  curl --fail --silent --show-error --max-time 20 \
    -X DELETE \
    -H "@$hdr" \
    "$identity_origin/v1/servers/$server_id" \
    || printf 'Directory deactivation failed. Retry that DELETE before deleting local files.\n' >&2
  rm -f "$hdr"
}

remove_created_files() {
  rm -f "$etc/identity.json" "$etc/directory.token" "$etc/enrollment.pending.json" "$dropin"
}

rollback_this_run() {
  deactivate_directory
  remove_created_files
  systemctl daemon-reload
  systemctl restart kiln-api.service
  printf 'Removed the enrollment files created by this run and restarted kiln-api only. Guest VMs were not deleted.\n' >&2
}

issue_door_certificate() {
  if [ -f "$etc/door.crt" ] && [ -f "$etc/door.key" ]; then
    return 0
  fi
  openssl req -new -newkey rsa:2048 -nodes \
    -subj "/CN=$public_host" \
    -keyout "$etc/door.key" -out "$etc/door.csr" >/dev/null 2>&1
  cat >"$etc/door.ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:$public_host
EOF
  local serial
  serial=0x$(openssl rand -hex 16)
  openssl x509 -req -in "$etc/door.csr" -CA "$etc/ca.crt" -CAkey "$etc/ca.key" \
    -set_serial "$serial" -days 365 -sha256 -extfile "$etc/door.ext" \
    -out "$etc/door.crt" >/dev/null 2>&1
  rm -f "$etc/door.csr" "$etc/door.ext"
  chown kiln-api:kiln-api "$etc/door.key" "$etc/door.crt"
  chmod 640 "$etc/door.key" "$etc/door.crt"
}

self_test() {
  command -v sqlite3 >/dev/null || die "sqlite3 is required"
  command -v openssl >/dev/null || die "openssl is required"
  command -v python3 >/dev/null || die "python3 is required"
  command -v curl >/dev/null || die "curl is required"
  if grep -nE 'Bearer \$[(]|Bearer \$\{' "$0"; then
    die "bearer token is expanded on a command line"
  fi
  local root token token_file hdr ready release portfile port pid cmdline py got saved_etc saved_db saved_dropin
  saved_etc=$etc
  saved_db=$db
  saved_dropin=$dropin
  root=$(mktemp -d)
  etc=$root/kiln
  db=$root/identity.db
  dropin=$root/identity.conf
  mkdir -p "$etc"
  assess_enrollment
  [ "$enrollment_state" = absent ]

  [ "$(plan_action absent 0)" = enroll ]
  [ "$(plan_action match 0)" = enable ]
  [ "$(plan_action match 1)" = enable ]
  [ "$(plan_action mismatch 0)" = refuse ]
  [ "$(plan_action incomplete 0)" = refuse ]
  [ "$(plan_action mismatch 1)" = rollback ]
  [ "$(plan_action incomplete 1)" = rollback ]

  sqlite3 "$db" <<'SQL'
CREATE TABLE provider_identities(provider TEXT, subject TEXT, account_id TEXT, display_name TEXT);
CREATE TABLE servers(id TEXT PRIMARY KEY, origin TEXT, active INTEGER);
INSERT INTO provider_identities VALUES ('github', '3084', 'owner_a', 'Jabbslad');
INSERT INTO servers VALUES ('server_b', 'https://cli.dark-forge.dev', 1);
SQL
  printf '%s\n' '{"version":1,"issuer":"https://dark-forge.dev","owner_id":"owner_a","server_id":"server_b"}' >"$etc/identity.json"
  assess_enrollment
  [ "$enrollment_state" = match ]
  [ "$subject" = 3084 ]

  sqlite3 "$db" "UPDATE servers SET origin='https://cli.dark-forge.dev:8443' WHERE id='server_b';"
  assess_enrollment
  [ "$enrollment_state" = mismatch ]
  sqlite3 "$db" "UPDATE servers SET origin='https://cli.dark-forge.dev/' WHERE id='server_b';"
  assess_enrollment
  [ "$enrollment_state" = mismatch ]
  sqlite3 "$db" "UPDATE servers SET origin='https://cli.dark-forge.dev', active=0 WHERE id='server_b';"
  assess_enrollment
  [ "$enrollment_state" = mismatch ]
  sqlite3 "$db" "UPDATE servers SET active=1 WHERE id='server_b';"
  sqlite3 "$db" "UPDATE provider_identities SET subject='9999' WHERE account_id='owner_a';"
  assess_enrollment
  [ "$enrollment_state" = mismatch ]
  sqlite3 "$db" "UPDATE provider_identities SET subject='3084', display_name='other' WHERE account_id='owner_a';"
  assess_enrollment
  [ "$enrollment_state" = mismatch ]
  sqlite3 "$db" "UPDATE provider_identities SET display_name='Jabbslad' WHERE account_id='owner_a';"
  sqlite3 "$db" "INSERT INTO provider_identities VALUES ('google', '3084', 'owner_a', 'Jabbslad');"
  assess_enrollment
  [ "$enrollment_state" = mismatch ]
  sqlite3 "$db" "DELETE FROM provider_identities WHERE provider='google';"
  assess_enrollment
  [ "$enrollment_state" = match ]

  rm -f "$etc/identity.json"
  printf '%s\n' secret >"$etc/directory.token"
  assess_enrollment
  [ "$enrollment_state" = incomplete ]
  token_before=$etc/directory.token
  [ -f "$token_before" ]
  remove_created_files
  [ ! -e "$etc/identity.json" ] && [ ! -e "$etc/directory.token" ] && [ ! -e "$dropin" ]

  printf '%s\n' keep >"$etc/identity.json"
  printf '%s\n' keep >"$etc/directory.token"
  [ "$(plan_action mismatch 0)" = refuse ]
  [ -f "$etc/identity.json" ] && [ -f "$etc/directory.token" ]

  token=$(openssl rand -hex 32)
  token_file=$(mktemp)
  hdr=$(mktemp)
  printf '%s' "$token" >"$token_file"
  write_bearer_header "$token_file" "$hdr"
  [ "$(stat -c %a "$hdr")" = 600 ]
  [ "$(cat "$hdr")" = "Authorization: Bearer $token" ]
  ready=$(mktemp)
  release=$(mktemp)
  portfile=$(mktemp)
  rm -f "$ready" "$release"
  python3 - "$portfile" "$ready" "$release" "$hdr.received" <<'PY' &
import os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
portfile, ready, release, out = sys.argv[1:]
class Handler(BaseHTTPRequestHandler):
    def do_DELETE(self):
        open(out, "w").write(self.headers.get("Authorization", ""))
        open(ready, "w").close()
        while not os.path.exists(release):
            time.sleep(0.05)
        self.send_response(204)
        self.end_headers()
    def log_message(self, fmt, *args):
        return
server = HTTPServer(("127.0.0.1", 0), Handler)
with open(portfile, "w") as handle:
    handle.write(str(server.server_address[1]))
    handle.flush()
server.handle_request()
PY
  py=$!
  for _ in $(seq 1 50); do
    [ -s "$portfile" ] && break
    sleep 0.05
  done
  port=$(cat "$portfile")
  curl --silent --show-error --max-time 10 -X DELETE -H "@$hdr" \
    "http://127.0.0.1:$port/v1/servers/server_b" &
  pid=$!
  for _ in $(seq 1 50); do
    [ -f "$ready" ] && break
    sleep 0.05
  done
  [ -f "$ready" ] || die "curl did not present the header file"
  cmdline=$(tr '\0' ' ' </proc/"$pid"/cmdline)
  case $cmdline in
    *"$token"*) die "directory token is visible on curl argv: $cmdline" ;;
  esac
  environ_text=$(tr '\0' '\n' </proc/"$pid"/environ || true)
  case $environ_text in
    *"$token"*) die "directory token is visible in curl's environment" ;;
  esac
  : >"$release"
  wait "$pid"
  wait "$py"
  got=$(cat "$hdr.received")
  [ "$got" = "Bearer $token" ] || die "curl did not send the bearer header"
  rm -f "$hdr" "$token_file" "$portfile" "$ready" "$release" "$hdr.received"
  etc=$saved_etc
  db=$saved_db
  dropin=$saved_dropin
  rm -rf "$root"
  printf 'enroll-dark-forge self-test passed\n'
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit 0
fi

[ "$(id -u)" -eq 0 ] || die "run this script as root"
[ "$#" -eq 1 ] || die "usage: enroll-dark-forge.sh PUBLIC_IPV4"
ip=$1
case $ip in
  *[!0-9.]*) die "PUBLIC_IPV4 must be a dotted IPv4 address" ;;
esac
[ "$ip" != "0.0.0.0" ] || die "the door must bind one address, not 0.0.0.0"
[ "${ip#127.}" = "$ip" ] || die "refusing a loopback address"
ip -o -4 addr show | awk '{print $4}' | awk -F/ '{print $1}' | grep -qx "$ip" \
  || die "$ip is not assigned on this host"
resolved=$(getent ahostsv4 "$public_host" | awk '{print $1; exit}')
[ "$resolved" = "$ip" ] || die "$public_host resolved to ${resolved:-nothing}, expected $ip"
if [ ! -f "$etc/ca.crt" ] || [ ! -f "$etc/ca.key" ]; then
  die "missing $etc/ca.crt or $etc/ca.key"
fi
[ -x "$libexec" ] || die "missing $libexec"
"$libexec" door --help >/dev/null || die "this kiln-api has no door command; install the release that contains it"
command -v sqlite3 >/dev/null || die "sqlite3 is required"
[ -f "$db" ] || die "missing $db"
argv=$(systemctl show kiln-api.service -p ExecStart --value)
case $argv in
  *"--listen 127.0.0.1:8443"*) ;;
  *) die "kiln-api must stay on 127.0.0.1:8443" ;;
esac
[ -f "$unit_src" ] || die "missing $unit_src"

assess_enrollment
action=$(plan_action "$enrollment_state" 0)
if [ "$action" = enroll ]; then
  printf 'Approve the next code at %s while signed in as GitHub user %s (id %s).\n' \
    "$identity_origin" "$expected_login" "$expected_subject"
  printf 'The page must show that GitHub login. The stored subject must be %s.\n' \
    "$expected_subject"
  "$libexec" enroll \
    --url "$expected_origin" \
    --local-url "https://127.0.0.1:8443" \
    --ca-file "$etc/ca.crt" \
    --name dark-forge
  assess_enrollment
  action=$(plan_action "$enrollment_state" 1)
fi
case $action in
  enable) ;;
  refuse)
    die "Existing enrollment does not match $expected_origin and GitHub user $expected_login subject $expected_subject. Files and kiln-door.service were left in place."
    ;;
  rollback)
    rollback_this_run
    die "This run's enrollment does not match $expected_origin and GitHub user $expected_login subject $expected_subject. Files created by this run were removed. Guest VMs were not deleted."
    ;;
  *)
    die "unexpected enrollment state"
    ;;
esac

issue_door_certificate
printf 'KILN_DOOR_LISTEN=127.0.0.1:8444\n' >"$etc/door.env"
chmod 644 "$etc/door.env"
install -m 644 "$unit_src" /etc/systemd/system/kiln-door.service
systemctl daemon-reload
systemctl enable kiln-door.service
systemctl restart kiln-door.service
# Caddy presents the public certificate. The private CA fails that handshake before the status is read.
code=$(curl --silent --show-error \
  --resolve "$public_host:443:$ip" --output /dev/null --write-out '%{http_code}' \
  "https://$public_host/v1/templates" || true)
[ "$code" = "401" ] || die "unauthenticated probe returned ${code:-none}"
printf 'Enrolled owner_id %s is GitHub subject %s display name %s.\n' \
  "$owner" "$subject" "$expected_login"
printf 'Stored server URL is %s.\n' "$expected_origin"
printf 'Unauthenticated https://%s/v1/templates returned 401.\n' "$public_host"
printf 'kiln-identity.service was not edited.\n'
printf 'This script does not change the firewall.\n'
printf 'On the laptop, install the client, then run kiln login, kiln list, kiln create, kiln exec, and kiln delete.\n'
printf 'Do not set KILN_IDENTITY_CA. kiln ssh uses the public door with an operate-scope owner JWT. The administrator token is not accepted there.\n'
