#!/bin/bash
# Laptop path: netns client -> public TLS door -> identity JWT -> loopback kiln-api -> guest.
# The GitHub stand-in needs a lab-provider build. Release kiln-identity has no substitute flags.
# https://dark-forge.dev refuses those URLs even with the feature.
set -euo pipefail
umask 077

repo=$(realpath "$(dirname "$0")/..")
work=$(mktemp -d /tmp/kiln-public-door.XXXXXX)
ns=kiln-laptop-$$
veth_host=k0$$
veth_ns=k1$$
door_ip=10.88.0.1
laptop_ip=10.88.0.2
ca=$work/ca.crt
hosts_line="$door_ip login.public.test cli.public.test github.public.test # kiln-public-door-$$"
pids=()

log() { printf '%s\n' "$*"; }
die() { printf 'public door test failed: %s\n' "$*" >&2; exit 1; }

cleanup() {
  status=$?
  set +e
  for pid in "${pids[@]+"${pids[@]}"}"; do
    kill "$pid" 2>/dev/null
  done
  sleep 0.2
  for pid in "${pids[@]+"${pids[@]}"}"; do
    kill -9 "$pid" 2>/dev/null
  done
  if [ -n "${host_pid:-}" ]; then kill "$host_pid" 2>/dev/null || true; fi
  sudo ip netns del "$ns" >/dev/null 2>&1 || true
  sudo ip link del "$veth_host" >/dev/null 2>&1 || true
  if [ -f /etc/hosts ]; then
    sudo sed -i "\#kiln-public-door-$$#d" /etc/hosts
  fi
  if [ "$status" -ne 0 ]; then
    printf 'work directory retained: %s\n' "$work" >&2
    for name in identity.log api.log door.log host.log github.log template.log login.log; do
      if [ -s "$work/$name" ]; then
        printf '%s\n' "--- $name ---" >&2
        tail -n 40 "$work/$name" >&2
      fi
    done
  else
    rm -rf "$work"
  fi
}
trap cleanup EXIT

track() { pids+=("$1"); }

need() { command -v "$1" >/dev/null || die "missing $1"; }
for tool in cargo openssl python3 curl sudo ip socat awk grep install sqlite3; do
  need "$tool"
done
sudo -n true || die "passwordless sudo is required"
log "checking enroll-dark-forge.sh"
bash "$repo/deploy/enroll-dark-forge.sh" --self-test

export PATH="$repo/.tools/firecracker-1.17.0:$PATH"
if [ ! -x "$repo/.tools/firecracker-1.17.0/firecracker" ]; then
  bash "$repo/scripts/fetch-firecracker.sh" "$repo/.tools/firecracker-1.17.0"
fi
if [ ! -x /usr/bin/busybox ]; then
  die "a static busybox is required at /usr/bin/busybox"
fi
sudo setfacl -m "u:$(id -un):rw" /dev/kvm
[ -r /dev/kvm ] && [ -w /dev/kvm ] || die "this user cannot use /dev/kvm"

log "checking the release identity binary has no substitute-provider flags"
cargo build --release --locked --manifest-path "$repo/Cargo.toml" \
  -p kiln-identity --bin kiln-identity >/dev/null
release_help=$("$repo/target/release/kiln-identity" --help)
case $release_help in
  *github-authorize-url*|*github-token-url*|*github-user-url*|*provider-ca-file*)
    die "release kiln-identity accepts a substitute provider"
    ;;
esac
log "release kiln-identity has no substitute-provider flags"

log "building binaries"
cargo build --locked --manifest-path "$repo/Cargo.toml" \
  --features kiln-identity/lab-provider \
  -p kiln-runtime --bin kiln-runtime \
  -p kiln-server --bin kiln-host --bin kiln-api \
  -p kiln-identity --bin kiln-identity \
  -p kiln-client --bin kiln \
  >/dev/null
runtime=$repo/target/debug/kiln-runtime
host_bin=$repo/target/debug/kiln-host
api_bin=$repo/target/debug/kiln-api
identity_bin=$repo/target/debug/kiln-identity
kiln=$repo/target/debug/kiln
case $("$identity_bin" --help) in
  *github-authorize-url*) ;;
  *) die "lab identity build is missing --github-authorize-url" ;;
esac

mkdir -m 0700 "$work/state" "$work/journal" "$work/run" "$work/home" "$work/bin" "$work/image"
printf '#!/bin/sh\nexit 1\n' >"$work/bin/xdg-open"
chmod 755 "$work/bin/xdg-open"
printf 'fixture-github-secret\n' >"$work/github.secret"
printf 'fixture-google-secret\n' >"$work/google.secret"
chmod 600 "$work/github.secret" "$work/google.secret"
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$work/signing.pem" >/dev/null 2>&1
chmod 600 "$work/signing.pem"
openssl rand -hex 32 >"$work/admin.token"
chmod 600 "$work/admin.token"

openssl req -x509 -newkey rsa:3072 -nodes -days 30 \
  -subj "/CN=kiln lab CA" \
  -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -keyout "$work/ca.key" -out "$ca" >/dev/null 2>&1
chmod 600 "$work/ca.key"

issue() {
  local name=$1 san=$2
  openssl req -new -newkey rsa:2048 -nodes -subj "/CN=$name" \
    -keyout "$work/$name.key" -out "$work/$name.csr" >/dev/null 2>&1
  cat >"$work/$name.ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=$san
EOF
  openssl x509 -req -in "$work/$name.csr" -CA "$ca" -CAkey "$work/ca.key" \
    -set_serial "0x$(openssl rand -hex 16)" -days 30 -sha256 \
    -extfile "$work/$name.ext" -out "$work/$name.crt" >/dev/null 2>&1
  chmod 600 "$work/$name.key"
}
issue login "DNS:login.public.test"
issue github "DNS:github.public.test"
issue door "DNS:cli.public.test"
issue api "IP:127.0.0.1"
cat "$work/login.crt" "$work/login.key" >"$work/login.pem"
chmod 600 "$work/login.pem"

cat >"$work/github_standin.py" <<'PY'
import json, ssl, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlparse

user_file, cert, key, host, port = sys.argv[1:]

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("%s\n" % (fmt % args))
    def user(self):
        return open(user_file, encoding="utf-8").read().strip()
    def send_json(self, body):
        raw = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/login/oauth/authorize":
            query = parse_qs(parsed.query)
            location = query["redirect_uri"][0]
            joiner = "&" if "?" in location else "?"
            target = location + joiner + urlencode({"code": self.user(), "state": query["state"][0]})
            self.send_response(302)
            self.send_header("Location", target)
            self.end_headers()
            return
        if parsed.path == "/user":
            login = self.user()
            self.send_json({"id": 3084 if login == "Jabbslad" else 1002, "login": login})
            return
        self.send_error(404)
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        if urlparse(self.path).path == "/login/oauth/access_token":
            self.send_json({"access_token": "fixture-token", "token_type": "bearer", "expires_in": 3600})
            return
        self.send_error(404)

context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
server = ThreadingHTTPServer((host, int(port)), Handler)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
PY

cat >"$work/ptyrun.py" <<'PY'
import os, sys
pid, fd = os.forkpty()
if pid == 0:
    os.execvpe(sys.argv[1], sys.argv[1:], os.environ)
while True:
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    sys.stdout.buffer.write(chunk)
    sys.stdout.buffer.flush()
_, status = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(status))
PY

printf '%s\n' Jabbslad >"$work/github-user"
sudo ip link add "$veth_host" type veth peer name "$veth_ns"
sudo ip addr add "$door_ip/24" dev "$veth_host"
sudo ip link set "$veth_host" up
sudo ip netns add "$ns"
sudo ip link set "$veth_ns" netns "$ns"
sudo ip netns exec "$ns" ip addr add "$laptop_ip/24" dev "$veth_ns"
sudo ip netns exec "$ns" ip link set "$veth_ns" up
sudo ip netns exec "$ns" ip link set lo up
printf '%s\n' "$hosts_line" | sudo tee -a /etc/hosts >/dev/null

python3 "$work/github_standin.py" "$work/github-user" "$work/github.crt" "$work/github.key" \
  "$door_ip" 8444 >"$work/github.log" 2>&1 &
track $!
"$identity_bin" \
  --listen 127.0.0.1:8091 \
  --public-origin https://login.public.test \
  --database "$work/identity.db" \
  --github-client-id fixture \
  --github-secret-file "$work/github.secret" \
  --google-client-id fixture-google \
  --google-secret-file "$work/google.secret" \
  --signing-key-file "$work/signing.pem" \
  --signing-key-id lab-key \
  --github-authorize-url https://github.public.test:8444/login/oauth/authorize \
  --github-token-url https://github.public.test:8444/login/oauth/access_token \
  --github-user-url https://github.public.test:8444/user \
  --provider-ca-file "$ca" \
  >"$work/identity.log" 2>&1 &
track $!
sudo socat "OPENSSL-LISTEN:443,bind=$door_ip,reuseaddr,fork,cert=$work/login.pem,verify=0" \
  TCP:127.0.0.1:8091 >"$work/proxy.log" 2>&1 &
track $!

ready_http() {
  local i
  for i in $(seq 1 50); do
    if curl --silent --cacert "$ca" --noproxy '*' --max-time 2 \
      "https://login.public.test/.well-known/jwks.json" -o "$work/jwks.json"; then
      return 0
    fi
    sleep 0.2
  done
  die "identity front door did not become ready"
}
ready_http
python3 - "$work/jwks.json" "$work/jwks-cache.json" <<'PY'
import json, sys
jwks = json.load(open(sys.argv[1]))
json.dump({"issuer": "https://login.public.test", "jwks": jwks}, open(sys.argv[2], "w"))
PY

curl_lab() {
  curl --silent --show-error --cacert "$ca" --noproxy '*' "$@"
}

approve() {
  local code=$1
  local login=$2
  local jar=$work/cookies-$code
  printf '%s\n' "$login" >"$work/github-user"
  rm -f "$jar"
  curl_lab -c "$jar" -D "$work/oauth.hdr" -o /dev/null \
    "https://login.public.test/oauth/github?user_code=$code"
  local location
  location=$(awk 'tolower($1)=="location:" {print $2}' "$work/oauth.hdr" | tr -d '\r')
  [ -n "$location" ] || die "oauth begin did not redirect"
  curl_lab -c "$jar" -b "$jar" -L -o "$work/callback.html" "$location"
  curl_lab -c "$jar" -b "$jar" -o "$work/device.html" \
    "https://login.public.test/device?user_code=$code"
  local csrf
  csrf=$(sed -n 's/.*name=csrf value="\([^"]*\)".*/\1/p' "$work/device.html" | head -n 1)
  [ -n "$csrf" ] || die "device page did not include a csrf field"
  curl_lab -c "$jar" -b "$jar" -o "$work/decision.html" \
    -H "Origin: https://login.public.test" \
    --data-urlencode "csrf=$csrf" \
    --data-urlencode "user_code=$code" \
    --data-urlencode "decision=approve" \
    "https://login.public.test/device/approve"
  grep -q "Decision saved" "$work/decision.html" || die "approval was not saved"
}

log "enrolling the lab server as Jabbslad"
ca_pem=$(python3 -c 'import json,sys; print(json.dumps(open(sys.argv[1]).read()))' "$ca")
curl_lab -o "$work/registration.json" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"dark-forge\",\"origin\":\"https://cli.public.test:8443\",\"ca_pem\":$ca_pem}" \
  https://login.public.test/v1/registrations
user_code=$(python3 -c 'import json; print(json.load(open("'"$work/registration.json"'"))["user_code"])')
reg_code=$(python3 -c 'import json; print(json.load(open("'"$work/registration.json"'"))["registration_code"])')
approve "$user_code" Jabbslad
for _ in $(seq 1 20); do
  sleep 6
  curl_lab -o "$work/enrolled.json" -H 'content-type: application/json' \
    -d "{\"registration_code\":\"$reg_code\"}" \
    https://login.public.test/v1/registrations/poll || true
  if python3 -c 'import json,sys; raise SystemExit("server_id" not in json.load(open(sys.argv[1])))' "$work/enrolled.json"; then
    break
  fi
done
python3 -c 'import json,sys; raise SystemExit("server_id" not in json.load(open(sys.argv[1])))' "$work/enrolled.json" \
  || die "enrollment poll did not complete"
owner_id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["owner_id"])' "$work/enrolled.json")
server_id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["server_id"])' "$work/enrolled.json")
directory_token=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["directory_token"])' "$work/enrolled.json")
python3 - "$work/identity.json" "$owner_id" "$server_id" <<'PY'
import json, sys
dest, owner, server = sys.argv[1:]
json.dump({"version": 1, "issuer": "https://login.public.test", "owner_id": owner, "server_id": server}, open(dest, "w"))
PY
sudo chown "root:$(id -gn)" "$work/identity.json"
sudo chmod 640 "$work/identity.json"
[ -r "$work/identity.json" ] || die "enrollment file is not readable by the API user"
subject=$(sqlite3 "$work/identity.db" \
  "SELECT subject FROM provider_identities WHERE account_id='$owner_id' AND provider='github' AND display_name='Jabbslad' AND subject='3084';")
extra=$(sqlite3 "$work/identity.db" \
  "SELECT count(*) FROM provider_identities WHERE account_id='$owner_id' AND NOT (provider='github' AND display_name='Jabbslad' AND subject='3084');")
[ "$subject" = 3084 ] && [ "$extra" = 0 ] || die "enrolled account was not GitHub user Jabbslad subject 3084"
log "enrolled owner $owner_id is GitHub subject $subject"

write_host() {
  python3 - "$work/host.json" "$work/state" "$work/journal" "$work/run/host.sock" "$1" <<'PY'
import json, sys
dest, runtime, journal, socket, template = sys.argv[1:]
templates = {"fixture": template} if template else {}
json.dump({
    "runtime_dir": runtime,
    "journal_dir": journal,
    "socket": socket,
    "allow_unsafe_development": True,
    "templates": templates,
}, open(dest, "w"))
PY
}
start_host() {
  if [ -n "${host_pid:-}" ]; then
    kill "$host_pid" 2>/dev/null || true
    wait "$host_pid" 2>/dev/null || true
  fi
  rm -f "$work/run/host.sock"
  "$host_bin" --config "$work/host.json" >"$work/host.log" 2>&1 &
  host_pid=$!
  local i
  for i in $(seq 1 50); do
    [ -S "$work/run/host.sock" ] && return 0
    sleep 0.1
  done
  die "host socket did not appear"
}
write_host ""
start_host
"$api_bin" \
  --listen 127.0.0.1:8443 \
  --host-socket "$work/run/host.sock" \
  --token-file "$work/admin.token" \
  --tls-cert "$work/api.crt" \
  --tls-key "$work/api.key" \
  --enrollment-file "$work/identity.json" \
  --jwks-cache "$work/jwks-cache.json" \
  >"$work/api.log" 2>&1 &
track $!
api_ready=0
for _ in $(seq 1 50); do
  code=$(curl --silent --cacert "$ca" --noproxy '*' --max-time 2 \
    -H "Authorization: Bearer $(tr -d '\n' <"$work/admin.token")" \
    -o "$work/templates.json" -w '%{http_code}' \
    https://127.0.0.1:8443/v1/templates || true)
  if [ "$code" = 200 ]; then
    api_ready=1
    break
  fi
  sleep 0.2
done
[ "$api_ready" = 1 ] || die "loopback API did not become ready"
curl_lab -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $directory_token" \
  https://login.public.test/v1/registrations/activate >"$work/activate.code"
grep -qx 204 "$work/activate.code" || die "activation returned $(cat "$work/activate.code")"
"$api_bin" door \
  --listen "$door_ip:8443" \
  --upstream https://127.0.0.1:8443 \
  --tls-cert "$work/door.crt" \
  --tls-key "$work/door.key" \
  --upstream-ca "$ca" \
  --enrollment-file "$work/identity.json" \
  --jwks-cache "$work/jwks-cache.json" \
  >"$work/door.log" 2>&1 &
track $!
door_ready=0
for _ in $(seq 1 50); do
  code=$(curl --silent --cacert "$ca" --noproxy '*' --max-time 2 \
    --resolve "cli.public.test:8443:$door_ip" \
    -o /dev/null -w '%{http_code}' \
    https://cli.public.test:8443/v1/templates || true)
  if [ "$code" != "000" ] && [ -n "$code" ]; then
    door_ready=1
    break
  fi
  sleep 0.1
done
[ "$door_ready" = 1 ] || die "public door did not listen"

api_sockets() {
  awk 'NR>1 && toupper($2)=="0100007F:20FB" { c++ } END { print c+0 }' /proc/net/tcp
}
expect_rejected() {
  local expected=$1
  shift
  local before after code
  before=$(api_sockets)
  code=$(curl --silent --show-error --cacert "$ca" --noproxy '*' \
    --resolve "cli.public.test:8443:$door_ip" \
    --output "$work/probe.body" --write-out '%{http_code}' \
    "$@" "https://cli.public.test:8443/v1/templates" || true)
  after=$(api_sockets)
  [ "$code" = "$expected" ] || die "door returned $code, wanted $expected ($(cat "$work/probe.body"))"
  [ "$before" = "$after" ] || die "loopback API sockets changed from $before to $after"
}
if sudo ip netns exec "$ns" curl --silent --max-time 2 --cacert "$ca" \
  https://127.0.0.1:8443/v1/templates >/dev/null; then
  die "client namespace reached loopback kiln-api"
fi
log "client namespace cannot dial 127.0.0.1:8443"
expect_rejected 401
log "unauthenticated request returned 401 without dialing kiln-api"
expect_rejected 401 -H "Authorization: Bearer $(tr -d '\n' <"$work/admin.token")"
log "administrator token returned 401 without dialing kiln-api"

log "logging in from the client namespace"
sudo ip netns exec "$ns" sudo -u "$(id -un)" \
  env HOME="$work/home" PATH="$work/bin:$PATH" KILN_IDENTITY_CA="$ca" \
  python3 "$work/ptyrun.py" \
  "$kiln" --config "$work/home/profiles.json" --issuer https://login.public.test login \
  >"$work/login.log" 2>&1 &
login_pid=$!
login_code=""
for _ in $(seq 1 40); do
  login_code=$(sed -n 's/.*enter code \([A-Z0-9]*\).*/\1/p' "$work/login.log" | head -n 1 || true)
  [ -n "$login_code" ] && break
  sleep 0.5
done
[ -n "$login_code" ] || die "kiln login did not print a code"
approve "$login_code" Jabbslad
wait "$login_pid" || die "kiln login failed"
log "kiln login succeeded"

laptop() {
  sudo ip netns exec "$ns" sudo -u "$(id -un)" \
    env HOME="$work/home" PATH="$PATH" KILN_IDENTITY_CA="$ca" \
    "$kiln" --config "$work/home/profiles.json" "$@"
}
laptop --json list >"$work/list.json"
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$work/list.json"
log "kiln list succeeded"

log "rejecting a second GitHub account"
curl_lab -o "$work/other-device.json" \
  -d 'client_id=kiln-cli&device_name=other laptop' \
  https://login.public.test/oauth/device/code
other_user=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["user_code"])' "$work/other-device.json")
other_device=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["device_code"])' "$work/other-device.json")
approve "$other_user" other
other_access=""
for _ in $(seq 1 15); do
  sleep 6
  curl_lab -o "$work/other-token.json" \
    -d "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code=$other_device" \
    https://login.public.test/oauth/token || true
  other_access=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("access_token",""))' "$work/other-token.json" || true)
  [ -n "$other_access" ] && break
done
[ -n "$other_access" ] || die "second account did not receive a device token"
denied=$(curl_lab -o "$work/denied.json" -w '%{http_code}' \
  -H "Authorization: Bearer $other_access" \
  -H 'content-type: application/json' \
  -d "{\"server_id\":\"$server_id\"}" \
  https://login.public.test/v1/access || true)
[ "$denied" = 404 ] || die "wrong account access returned $denied"
curl_lab -o "$work/other-registration.json" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"other\",\"origin\":\"https://other.public.test/\",\"ca_pem\":$ca_pem}" \
  https://login.public.test/v1/registrations
other_reg_user=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["user_code"])' "$work/other-registration.json")
other_reg_code=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["registration_code"])' "$work/other-registration.json")
approve "$other_reg_user" other
other_server=""
for _ in $(seq 1 20); do
  sleep 6
  curl_lab -o "$work/other-enrolled.json" -H 'content-type: application/json' \
    -d "{\"registration_code\":\"$other_reg_code\"}" \
    https://login.public.test/v1/registrations/poll || true
  other_server=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("server_id",""))' "$work/other-enrolled.json" || true)
  [ -n "$other_server" ] && break
done
[ -n "$other_server" ] || die "second enrollment did not complete"
other_directory=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["directory_token"])' "$work/other-enrolled.json")
curl_lab -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $other_directory" \
  https://login.public.test/v1/registrations/activate | grep -qx 204
other_jwt=$(curl_lab -H "Authorization: Bearer $other_access" -H 'content-type: application/json' \
  -d "{\"server_id\":\"$other_server\"}" \
  https://login.public.test/v1/access)
other_jwt=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["access_token"])' "$other_jwt")
expect_rejected 401 -H "Authorization: Bearer $other_jwt"
log "wrong-account token returned 401 without dialing kiln-api"

log "building a guest template"
if [ ! -f "$work/image/image.json" ]; then
  bash "$repo/images/build-fixture.sh" "$work/image" /usr/bin/busybox
fi
kill "$host_pid"
wait "$host_pid" || true
host_pid=""
if ! "$runtime" --state-dir "$work/state" template build \
  --image "$work/image/image.json" \
  --memory-mib 256 --vcpus 1 \
  --profile development --allow-unsafe-development \
  >"$work/template.json" 2>"$work/template.log"; then
  die "template build failed: $(cat "$work/template.log")"
fi
template_id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$work/template.json")
write_host "$template_id"
start_host
api_ready=0
for _ in $(seq 1 50); do
  code=$(curl --silent --cacert "$ca" --noproxy '*' --max-time 2 \
    -H "Authorization: Bearer $(tr -d '\n' <"$work/admin.token")" \
    -o "$work/templates.json" -w '%{http_code}' \
    https://127.0.0.1:8443/v1/templates || true)
  if [ "$code" = 200 ]; then
    api_ready=1
    break
  fi
  sleep 0.2
done
[ "$api_ready" = 1 ] || die "API did not recover after the template build"
laptop --json create --name public-door >"$work/create.json"
box=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["box_id"])' "$work/create.json")
[ -n "$box" ] || die "create did not return a box"
exec_out=$(laptop exec "$box" -- /bin/echo public-door)
[ "$exec_out" = "public-door" ] || die "exec returned $(printf '%s' "$exec_out")"
laptop delete "$box" >"$work/delete.out"
laptop --json list >"$work/list-after.json"
python3 -c 'import json,sys; ids=[item["id"] for item in json.load(open(sys.argv[1]))]; raise SystemExit(sys.argv[2] in ids)' \
  "$work/list-after.json" "$box"
log "public door laptop path passed"
