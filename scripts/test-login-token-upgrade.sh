#!/bin/bash
# Upgrade a host laid down from a published Kiln release that has no login.token.
# The release archives are real GitHub release assets. This does not use a
# dark-forge fixture and it does not print secret material.
# The lab CA is compiled only into this test's debug kiln-api
# (--features loopback-identity-ca). Release binaries do not include it.
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
script=$root/scripts/test-login-token-upgrade.sh
repo=${GH_REPO:-Jabbslad/kiln}
issuer=https://127.0.0.1:18443
identity_port=8091

fail() { printf 'login-token-upgrade: %s\n' "$*" >&2; exit 1; }

case "${KILN_SERVER_UPGRADE_PREV_TAR:-}${KILN_LOGIN_UPGRADE_WORK:-}$script" in
  *dark-forge*) fail "refusing a dark-forge path" ;;
esac

if [ "${1:-}" = --root ]; then
  [ "$(id -u)" -eq 0 ] || fail "root phase is not root"
  work=${KILN_LOGIN_UPGRADE_WORK:-}
  if [ -z "$work" ] || [ ! -d "$work" ]; then
    fail "missing upgrade work directory"
  fi
  # shellcheck source=/dev/null
  . "$work/root-phase.sh"
  exit 0
fi

for tool in gh jq curl openssl sqlite3 caddy sudo unshare sha256sum cargo; do
  command -v "$tool" >/dev/null || fail "missing $tool"
done
if [ -n "${GH_TOKEN:-${GITHUB_TOKEN:-}}" ]; then
  export GH_TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
elif ! gh auth status >/dev/null 2>&1; then
  fail "gh is not authenticated"
fi

workspace_version=$(awk '
  $1 == "version" && $2 == "=" {
    gsub(/"/, "", $3)
    print $3
    exit
  }
' "$root/Cargo.toml")
printf '%s\n' "$workspace_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "workspace version is not x.y.z"

version_le() {
  # Return 0 when $1 <= $2, both dotted triples.
  local a=$1 b=$2
  [ "$a" = "$b" ] && return 0
  local older
  older=$(printf '%s\n%s\n' "$a" "$b" | sort -t. -k1,1n -k2,2n -k3,3n | head -1)
  [ "$older" = "$a" ]
}

work=$(mktemp -d)
chmod 700 "$work"
pids=()
cleanup() {
  local pid
  for pid in "${pids[@]+"${pids[@]}"}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
  if [ -n "${work:-}" ] && [ -d "$work" ]; then
    sudo -n rm -rf "$work" || rm -rf "$work" || true
  fi
}
trap cleanup EXIT

download_release() {
  local tag=$1 name
  mkdir -p "$work/assets"
  for name in \
    "kiln-server-${tag}-x86_64-unknown-linux-gnu.tar.gz" \
    "kiln-identity-${tag}-x86_64-unknown-linux-gnu.tar.gz"
  do
    case $name in
      *dark-forge*) fail "refusing a dark-forge asset" ;;
    esac
    gh release download "$tag" --repo "$repo" --pattern "$name" --pattern "${name}.sha256" \
      --dir "$work/assets" --clobber
    [ -f "$work/assets/$name" ] || fail "release $tag did not include $name"
    [ -f "$work/assets/${name}.sha256" ] || fail "release $tag did not include ${name}.sha256"
  done
  (
    cd "$work/assets"
    sha256sum -c "kiln-server-${tag}-x86_64-unknown-linux-gnu.tar.gz.sha256"
    sha256sum -c "kiln-identity-${tag}-x86_64-unknown-linux-gnu.tar.gz.sha256"
  ) >/dev/null
}

pick_tag() {
  local tags tag version preferred=v${workspace_version}
  if gh release view "$preferred" --repo "$repo" --json assets \
    --jq '.assets[].name' 2>/dev/null \
    | grep -qx "kiln-server-${preferred}-x86_64-unknown-linux-gnu.tar.gz" \
    && gh release view "$preferred" --repo "$repo" --json assets \
      --jq '.assets[].name' | grep -qx "kiln-identity-${preferred}-x86_64-unknown-linux-gnu.tar.gz"
  then
    printf '%s\n' "$preferred"
    return 0
  fi
  tags=$(gh release list --repo "$repo" --exclude-drafts --exclude-pre-releases --limit 100 \
    | awk '{print $1}')
  [ -n "$tags" ] || fail "no published releases"
  while IFS= read -r tag; do
    printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || continue
    version=${tag#v}
    version_le "$version" "$workspace_version" || continue
    if gh release view "$tag" --repo "$repo" --json assets \
      --jq '.assets[].name' | grep -qx "kiln-server-${tag}-x86_64-unknown-linux-gnu.tar.gz" \
      && gh release view "$tag" --repo "$repo" --json assets \
        --jq '.assets[].name' | grep -qx "kiln-identity-${tag}-x86_64-unknown-linux-gnu.tar.gz"
    then
      printf '%s\n' "$tag"
      return 0
    fi
  done <<<"$tags"
  fail "no release at or below $workspace_version has server and identity archives"
}

tag=$(pick_tag)
download_release "$tag"
installed_version=${tag#v}

mkdir -p "$work/prev" "$work/identity-pkg"
tar -xzf "$work/assets/kiln-server-${tag}-x86_64-unknown-linux-gnu.tar.gz" -C "$work/prev"
tar -xzf "$work/assets/kiln-identity-${tag}-x86_64-unknown-linux-gnu.tar.gz" -C "$work/identity-pkg"
[ -x "$work/identity-pkg/bin/kiln-identity" ] || chmod 755 "$work/identity-pkg/bin/kiln-identity"
for name in kiln kiln-runtime kiln-network kiln-host kiln-api; do
  [ -f "$work/prev/bin/$name" ] || fail "server archive is missing bin/$name"
done

cargo build --locked --bins -p kiln-server -p kiln-runtime -p kiln-client >/dev/null
for name in kiln kiln-runtime kiln-host kiln-api; do
  [ -x "$root/target/debug/$name" ] || fail "missing workspace binary $name"
done
if grep -a -F -q 'loopback-identity-ca.pem' "$root/target/debug/kiln-api"; then
  fail "workspace kiln-api contains the lab CA path"
fi
cargo build --locked --bin kiln-api -p kiln-server \
  --features loopback-identity-ca \
  --target-dir "$work/ca-feature" >/dev/null
[ -x "$work/ca-feature/debug/kiln-api" ] || fail "missing feature kiln-api"
grep -a -F -q 'loopback-identity-ca.pem' "$work/ca-feature/debug/kiln-api" \
  || fail "upgrade kiln-api was built without the lab CA"

mkdir -p "$work/id/secrets"
chmod 700 "$work/id" "$work/id/secrets"
# rustls only trusts a certificate with the CA bit. Caddy presents the leaf.
if ! openssl req -x509 -newkey rsa:2048 -keyout "$work/id/ca.key" -out "$work/id/ca.pem" \
  -days 1 -nodes -subj /CN=kiln-upgrade-ca \
  -addext basicConstraints=critical,CA:TRUE \
  -addext keyUsage=critical,keyCertSign,cRLSign >/dev/null 2>&1 \
  || ! openssl req -newkey rsa:2048 -keyout "$work/id/key.pem" -out "$work/id/csr.pem" \
    -nodes -subj /CN=127.0.0.1 \
    -addext subjectAltName=IP:127.0.0.1 \
    -addext basicConstraints=CA:FALSE \
    -addext keyUsage=critical,digitalSignature,keyEncipherment \
    -addext extendedKeyUsage=serverAuth >/dev/null 2>&1 \
  || ! openssl x509 -req -in "$work/id/csr.pem" -CA "$work/id/ca.pem" -CAkey "$work/id/ca.key" \
    -CAcreateserial -out "$work/id/cert.pem" -days 1 -copy_extensions copy >/dev/null 2>&1
then
  fail "could not mint the loopback certificate"
fi
chmod 600 "$work/id/ca.key" "$work/id/key.pem" "$work/id/csr.pem"
chmod 644 "$work/id/ca.pem" "$work/id/cert.pem"
openssl rand -hex 16 > "$work/id/secrets/github"
openssl rand -hex 16 > "$work/id/secrets/google"
chmod 600 "$work/id/secrets/github" "$work/id/secrets/google"
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$work/id/secrets/signing.pem" \
  >/dev/null 2>&1
chmod 600 "$work/id/secrets/signing.pem"
openssl req -x509 -newkey rsa:2048 -keyout "$work/id/upstream.key" -out "$work/id/upstream.pem" \
  -days 1 -nodes -subj /CN=127.0.0.1 >/dev/null 2>&1
chmod 644 "$work/id/upstream.pem"

cat > "$work/id/Caddyfile" << EOF
{
  admin off
  auto_https off
}
https://127.0.0.1:18443 {
  tls $work/id/cert.pem $work/id/key.pem
  reverse_proxy 127.0.0.1:${identity_port}
}
EOF

"$work/identity-pkg/bin/kiln-identity" \
  --listen "127.0.0.1:${identity_port}" \
  --public-origin "$issuer" \
  --database "$work/id/identity.db" \
  --github-client-id kiln-upgrade-test \
  --github-secret-file "$work/id/secrets/github" \
  --google-client-id kiln-upgrade-test \
  --google-secret-file "$work/id/secrets/google" \
  --signing-key-file "$work/id/secrets/signing.pem" \
  --signing-key-id kiln-upgrade-test \
  >"$work/identity.log" 2>&1 &
pids+=("$!")
caddy run --config "$work/id/Caddyfile" >"$work/caddy.log" 2>&1 &
pids+=("$!")

ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl -fsS "http://127.0.0.1:${identity_port}/.well-known/jwks.json" -o "$work/jwks.json" 2>/dev/null \
    && curl -fsS --cacert "$work/id/ca.pem" "${issuer}/.well-known/jwks.json" -o "$work/jwks-https.json" 2>/dev/null
  then
    ready=1
    break
  fi
  sleep 0.2
done
if [ "$ready" != 1 ]; then
  cat "$work/identity.log" >&2 || true
  cat "$work/caddy.log" >&2 || true
  fail "identity did not accept connections"
fi

sqlite3 "$work/id/identity.db" ".timeout 5000" \
  "INSERT INTO accounts(id,created_at,revoked_at) VALUES('acct_login_upgrade', strftime('%s','now'), NULL);"

jq -n --arg name old-install --arg origin "https://127.0.0.1:8443" --rawfile ca "$work/id/upstream.pem" \
  '{name:$name,origin:$origin,ca_pem:$ca}' > "$work/register.json"
reg_status=$(curl -sS -o "$work/reg.json" -w '%{http_code}' \
  -H 'content-type: application/json' \
  --data-binary @"$work/register.json" \
  "http://127.0.0.1:${identity_port}/v1/registrations")
[ "$reg_status" = 200 ] || fail "registration status $reg_status"
jq -e -r '.user_code' "$work/reg.json" > "$work/user_code"
jq -c '{registration_code: .registration_code}' "$work/reg.json" > "$work/poll-body.json"
chmod 600 "$work/user_code" "$work/poll-body.json" "$work/reg.json"
rm -f "$work/reg.json"

approved=$(sqlite3 "$work/id/identity.db" ".timeout 5000" \
  "UPDATE pending_registrations SET state='approved', owner_id='acct_login_upgrade' WHERE user_code='$(cat "$work/user_code")' AND state='pending'; SELECT changes();")
[ "$approved" = 1 ] || fail "registration was not approved"
rm -f "$work/user_code"

# The first legal poll is five seconds after registration. An early poll
# lengthens that wait, so this sleeps once before asking.
sleep 6
polled=0
for _ in 1 2 3 4; do
  poll_status=$(curl -sS -o "$work/poll.json" -w '%{http_code}' \
    -H 'content-type: application/json' \
    --data-binary @"$work/poll-body.json" \
    "http://127.0.0.1:${identity_port}/v1/registrations/poll" || true)
  if [ "$poll_status" = 200 ]; then
    polled=1
    break
  fi
  [ "$poll_status" = 429 ] || fail "registration poll status $poll_status"
  sleep 12
done
[ "$polled" = 1 ] || fail "registration poll did not finish"
jq -e -r '.server_id' "$work/poll.json" > "$work/server_id"
jq -e -r '.owner_id' "$work/poll.json" > "$work/owner_id"
jq -j '.directory_token' "$work/poll.json" > "$work/directory.token"
chmod 600 "$work/server_id" "$work/owner_id" "$work/directory.token"
rm -f "$work/poll.json" "$work/poll-body.json"
[ -s "$work/directory.token" ] || fail "registration did not return a directory file"
if [ "$(tr -cd '\n\r' < "$work/directory.token" | wc -c | tr -d '[:space:]')" -ne 0 ]; then
  fail "directory file contains a newline"
fi

activate_status=$(
  awk 'NR == 1 { printf "Authorization: Bearer %s\n", $0 }' "$work/directory.token" |
    curl -sS -o /dev/null -w '%{http_code}' -H @- -X POST \
      "http://127.0.0.1:${identity_port}/v1/registrations/activate"
)
[ "$activate_status" = 204 ] || fail "activation status $activate_status"

jq -n --arg issuer "$issuer" --arg owner "$(cat "$work/owner_id")" --arg server "$(cat "$work/server_id")" \
  '{version:1,issuer:$issuer,owner_id:$owner,server_id:$server}' > "$work/identity.json"

session=$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=\n')
session_hash=$(printf '%s' "$session" | sha256sum | awk '{print $1}')
printf '%s' "$session" > "$work/session"
chmod 600 "$work/session"
unset session
exp=$(( $(date +%s) + 86400 ))
sqlite3 "$work/id/identity.db" ".timeout 5000" \
  "INSERT INTO browser_sessions(hash,csrf_hash,account_id,expires_at) VALUES(x'${session_hash}', x'${session_hash}', 'acct_login_upgrade', ${exp});"
unset session_hash

cat > "$work/root-phase.sh" << 'EOF'
# Loaded only inside the mount namespace. Paths come from the work directory.
set -euo pipefail
fail() { printf 'login-token-upgrade: %s\n' "$*" >&2; exit 1; }
work=$KILN_LOGIN_UPGRADE_WORK
root=$(cat "$work/repo-root")
tag=$(cat "$work/tag")
installed_version=$(cat "$work/installed-version")
issuer=$(cat "$work/issuer")

mount --make-rprivate /
etc_created=0
if [ ! -d /etc/kiln ]; then
  mkdir -p /etc/kiln
  etc_created=1
fi
mount --bind "$work/etc/kiln" /etc/kiln
chown root:root /etc/kiln
chmod 755 /etc/kiln
cleanup_mount() {
  if [ -n "${app_pid:-}" ]; then
    pkill -P "$app_pid" >/dev/null 2>&1 || true
    kill "$app_pid" >/dev/null 2>&1 || true
    wait "$app_pid" 2>/dev/null || true
  fi
  umount /etc/kiln >/dev/null 2>&1 || true
  if [ "$etc_created" = 1 ]; then
    rmdir /etc/kiln >/dev/null 2>&1 || true
  fi
}
trap cleanup_mount EXIT

getent group kiln-api >/dev/null || groupadd --system kiln-api
if ! id kiln-api >/dev/null 2>&1; then
  useradd --system --gid kiln-api --home-dir /nonexistent --shell /usr/sbin/nologin kiln-api
fi
id -nG kiln-api | tr ' ' '\n' | grep -qx kiln-api || usermod -a -G kiln-api kiln-api
gid=$(getent group kiln-api | awk -F: '{print $3}')

install -m 600 -o root -g root "$work/directory.token" /etc/kiln/directory.token
install -m 640 -o root -g kiln-api "$work/identity.json" /etc/kiln/identity.json
install -m 600 -o root -g root "$work/id/ca.pem" /etc/kiln/loopback-identity-ca.pem
if [ -e /etc/kiln/login.token ] || [ -L /etc/kiln/login.token ]; then
  fail "old install already had a login file"
fi

state=$work/stub-state
log=$work/commands.log
package=$work/package
etc=$work/etc/kiln
opt=$work/opt/kiln
var=$work/var/lib/kiln
libexec=$work/usr/local/libexec
mkdir -p "$state" "$package/bin" "$etc" "$opt/bin" "$var/runtime" "$var/journal" "$var/image" \
  "$libexec" "$work/run/systemd/system" "$work/sys/fs/cgroup" "$work/dev" \
  "$work/etc/systemd/system" "$work/proc" "$work/stubs" "$work/run/lock" "$opt/upgrades"
printf 'ID=ubuntu\nVERSION_ID=24.04\n' > "$work/etc/os-release"
printf 'MemAvailable:       8388608 kB\n' > "$work/proc/meminfo"
printf 'cpu memory pids\n' > "$work/sys/fs/cgroup/cgroup.controllers"
: > "$work/dev/kvm"
printf 'process\n' > "$state/killmode"
: > "$log"
cat > "$work/stubs/systemctl" << STUB
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
STUB
cat > "$work/stubs/curl" << STUB
#!/bin/bash
if [ ! -t 0 ]; then cat >/dev/null; fi
printf '%s\\n' "curl \$*" >> "$log"
if [ ! -s "$work/curl.seq" ]; then exit 1; fi
line=\$(head -n 1 "$work/curl.seq")
tail -n +2 "$work/curl.seq" > "$work/curl.seq.next"
mv "$work/curl.seq.next" "$work/curl.seq"
[ "\$line" = ok ] || exit 1
printf '%s' '[{"name":"custom","memory_mib":3072,"vcpus":2}]'
STUB
printf '#!/bin/bash\nexit 0\n' > "$work/stubs/systemd-analyze"
chmod 755 "$work/stubs/systemctl" "$work/stubs/curl" "$work/stubs/systemd-analyze"
cp "$root/deploy/install.sh" "$package/install.sh"
cp "$root/deploy/operations-journal.max-version" "$package/operations-journal.max-version"
chmod 755 "$package/install.sh"
custom=$(printf 'a%.0s' $(seq 1 32))
printf '{"runtime_dir":"%s/runtime","journal_dir":"%s/journal","socket":"/run/kiln/host.sock","isolation_config":"%s/isolation.json","templates":{"custom":"%s"}}\n' \
  "$var" "$var" "$etc" "$custom" > "$etc/host.json"
printf '{}\n' > "$etc/isolation.json"
for name in ca.crt tls.crt tls.key admin.token; do printf 'synthetic-test-only\n' > "$etc/$name"; done
sqlite3 "$var/journal/operations.sqlite" \
  'PRAGMA journal_mode=DELETE; PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);' >/dev/null
for name in kiln kiln-runtime kiln-network kiln-host kiln-api; do
  if [ "$name" = kiln ] || [ "$name" = kiln-runtime ] || [ "$name" = kiln-network ]; then
    dest=$opt/bin/$name
  else
    dest=$libexec/$name
  fi
  if [ "$name" = kiln-network ]; then
    cp "$work/prev/bin/kiln-network" "$dest"
    cp "$root/deploy/kiln-network" "$package/bin/kiln-network"
  elif [ "$name" = kiln ] || [ "$name" = kiln-runtime ]; then
    cp "$work/prev/bin/$name" "$dest"
    cp "$root/target/debug/$name" "$package/bin/$name"
  elif [ "$name" = kiln-api ]; then
    cp "$work/prev/bin/$name" "$dest"
    cp "$work/ca-feature/debug/kiln-api" "$package/bin/$name"
  else
    cp "$work/prev/bin/$name" "$dest"
    cp "$root/target/debug/$name" "$package/bin/$name"
  fi
  chmod 755 "$dest" "$package/bin/$name"
  chown root:root "$dest" "$package/bin/$name"
done
# The published archive and this workspace share a Cargo version, so an
# unstamped package reports the same version as the install. Equal versions
# are still an upgrade when the bytes differ, and they are "already current"
# when the bytes match. Stamp the new package one patch above the installed
# release so a same-version run cannot pass. The installer accepts only X.Y.Z.
installed_reported=$("$libexec/kiln-api" --version | awk '{print $2}')
[ "$installed_reported" = "$installed_version" ] || fail "installed kiln-api reports ${installed_reported}"
target_version=$(printf '%s\n' "$installed_version" | awk -F. '{ printf "%d.%d.%d\n", $1, $2, $3 + 1 }')
[ "$installed_version" != "$target_version" ] || fail "same-version package is not an upgrade"
for name in kiln kiln-runtime kiln-host kiln-api; do
  sh "$root/scripts/stamp-release-version.sh" "$package/bin/$name" "$target_version"
  case $name in
    kiln-host|kiln-api) reported=kiln-server ;;
    *) reported=$name ;;
  esac
  got=$("$package/bin/$name" --version)
  [ "$got" = "$reported $target_version" ] || fail "$name reports ${got}"
done
printf '{"version":"v%s","target":"x86_64-unknown-linux-gnu","min_upgrade_from":"%s"}\n' \
  "$target_version" "$installed_version" > "$package/release.json"
cp "$work/prev/release.json" "$opt/release.json"
host=$libexec/kiln-host
api=$libexec/kiln-api
printf '{ path=%s ; argv[]=%s --config %s ; ignore_errors=no ; }\n' "$host" "$host" "$etc/host.json" > "$state/exec-kiln-host.service"
printf '{ path=%s ; argv[]=%s --listen 100.64.5.6:8443 --host-socket /run/kiln/host.sock --token-file %s --tls-cert %s --tls-key %s ; ignore_errors=no ; }\n' \
  "$api" "$api" "$etc/admin.token" "$etc/tls.crt" "$etc/tls.key" > "$state/exec-kiln-api.service"
printf 'active\n' > "$state/active-kiln-host.service"
printf 'active\n' > "$state/active-kiln-api.service"
printf 'inactive\n' > "$state/active-kiln-app.service"

stamp() {
  local path=$1
  stat -c '%u %g %a %Y %s' "$path"
  sha256sum "$path" | awk '{print $1}'
}

leaked() {
  local needle
  for needle in "$work/directory.token" /etc/kiln/login.token; do
    [ -s "$needle" ] || continue
    if grep -F -q -f "$needle" "$work/out" "$work/err" "$log"; then
      return 0
    fi
  done
  return 1
}

run_upgrade() {
  local curls=$1
  : > "$work/curl.seq"
  local i=0
  while [ "$i" -lt "$curls" ]; do
    printf 'ok\n' >> "$work/curl.seq"
    i=$((i + 1))
  done
  set +e
  KILN_INSTALL_FIXTURE=$work PATH="$work/stubs:$PATH" bash "$package/install.sh" --apply \
    >"$work/out" 2>"$work/err"
  rc=$?
  set -e
}

reach=$(curl -sS -o /dev/null -w '%{http_code}' --cacert "$work/id/ca.pem" \
  "${issuer}/.well-known/jwks.json" || true)
[ "$reach" = 200 ] || fail "loopback issuer status ${reach}"

run_upgrade 2
if [ "$rc" != 0 ]; then
  leaked && fail "upgrade output contained a secret"
  cat "$work/err" >&2 || true
  fail "upgrade from ${tag} did not publish (${installed_version} -> ${target_version})"
fi
grep -F -q "Kiln ${installed_version} -> ${target_version}:" "$work/out" \
  || fail "upgrade did not swap binaries"
if grep -F -q 'already current' "$work/out"; then
  fail "first upgrade treated the package as already current"
fi
[ -f /etc/kiln/login.token ] || fail "missing login file was not created"
[ ! -L /etc/kiln/login.token ] || fail "login file is a symlink"
if [ "$(tr -cd '\n\r' < /etc/kiln/login.token | wc -c | tr -d '[:space:]')" -ne 0 ]; then
  fail "login file contains a newline"
fi
meta=$(stat -c '%u %g %a' /etc/kiln/login.token)
[ "$meta" = "0 ${gid} 640" ] || fail "login file owner or mode is ${meta}"
before=$(stamp /etc/kiln/login.token)
leaked && fail "upgrade output contained a secret"

run_upgrade 1
if [ "$rc" != 0 ]; then
  leaked && fail "second upgrade output contained a secret"
  cat "$work/err" >&2 || true
  fail "second upgrade should leave the login file in place"
fi
after=$(stamp /etc/kiln/login.token)
[ "$before" = "$after" ] || fail "existing login file changed"
grep -F -q "Kiln ${target_version} is already current" "$work/out" \
  || fail "second upgrade did not see the swapped binaries"
leaked && fail "second upgrade output contained a secret"

cache=$work/jwks-cache.json
jq -n --arg issuer "$issuer" --slurpfile jwks "$work/jwks-https.json" \
  '{issuer:$issuer,jwks:$jwks[0]}' > "$cache"
chown kiln-api:kiln-api "$cache"
chmod 600 "$cache"
chmod 644 "$work/id/upstream.pem"
# The fixture tree is root-owned. kiln-api only needs to traverse to its
# binary, the upstream CA, and this cache. Secret files stay owner-only.
chmod 711 "$work" "$work/usr" "$work/usr/local" "$work/usr/local/libexec" "$work/id"

runuser -u kiln-api -- "$libexec/kiln-api" app \
  --upstream https://127.0.0.1:8443 \
  --upstream-ca "$work/id/upstream.pem" \
  --enrollment-file /etc/kiln/identity.json \
  --jwks-cache "$cache" \
  >"$work/app.log" 2>&1 &
app_pid=$!
listened=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if grep -q 'app listener listening on 127.0.0.1:8093' "$work/app.log"; then
    listened=1
    break
  fi
  if ! kill -0 "$app_pid" 2>/dev/null; then
    break
  fi
  sleep 0.2
done
if [ "$listened" != 1 ]; then
  if [ -s /etc/kiln/login.token ] && grep -F -q -f /etc/kiln/login.token "$work/app.log"; then
    fail "app log contained a secret"
  fi
  cat "$work/app.log" >&2 || true
  fail "dashboard listener did not start"
fi

login_status=$(curl -sS -D "$work/login.headers" -o "$work/login.body" -w '%{http_code}' \
  "http://127.0.0.1:8093/login")
case $login_status in
  302|303) ;;
  *) fail "dashboard login status ${login_status}" ;;
esac
if grep -q 'login_unavailable' "$work/login.body"; then
  fail "dashboard sign-in is still unavailable"
fi
loc=$(awk 'tolower($1) == "location:" { print $2 }' "$work/login.headers" | tr -d '\r')
case $loc in
  "${issuer}/oauth/server?"*) ;;
  *) fail "dashboard login did not redirect to the issuer" ;;
esac
attempt=$(awk '
  tolower($1) == "set-cookie:" && $0 ~ /__Host-kiln_app_attempt=/ {
    line = $0
    sub(/.*__Host-kiln_app_attempt=/, "", line)
    sub(/;.*/, "", line)
    gsub(/\r/, "", line)
    print line
    exit
  }
' "$work/login.headers")
[ -n "$attempt" ] || fail "dashboard login did not start an attempt"
query=${loc#*\?}
oauth_status=$(curl -sS --cacert "$work/id/ca.pem" -D "$work/oauth.headers" -o /dev/null -w '%{http_code}' \
  -H "Cookie: __Host-kiln_session=$(cat "$work/session")" \
  "https://127.0.0.1:18443/oauth/server?${query}")
case $oauth_status in
  302|303) ;;
  *) fail "identity login status ${oauth_status}" ;;
esac
callback=$(awk 'tolower($1) == "location:" { print $2 }' "$work/oauth.headers" | tr -d '\r')
code=$(printf '%s\n' "$callback" | sed -n 's/.*[?&]code=\([^&]*\).*/\1/p')
state=$(printf '%s\n' "$callback" | sed -n 's/.*[?&]state=\([^&]*\).*/\1/p')
[ -n "$code" ] && [ -n "$state" ] || fail "identity did not return a login code"
finish_status=$(curl -sS -D "$work/finish.headers" -o /dev/null -w '%{http_code}' \
  -H "Cookie: __Host-kiln_app_attempt=${attempt}" \
  "http://127.0.0.1:8093/oauth/callback?code=${code}&state=${state}")
case $finish_status in
  302|303) ;;
  *) fail "dashboard login finish status ${finish_status}" ;;
esac
finish_loc=$(awk 'tolower($1) == "location:" { print $2 }' "$work/finish.headers" | tr -d '\r')
case $finish_loc in
  /boxes|/boxes/) ;;
  *)
    safe=${finish_loc%%\?*}
    case ${#safe} in
      [0-9]|[1-9][0-9]|1[0-9][0-9]) ;;
      *) safe=long ;;
    esac
    fail "dashboard login did not finish (${safe:-empty})"
    ;;
esac
awk '
  tolower($1) == "set-cookie:" {
    if ($0 ~ /__Host-kiln_app_attempt=/) next
    if ($0 ~ /__Host-kiln_app=/) found = 1
  }
  END { exit !found }
' "$work/finish.headers" || fail "dashboard login did not set a session"
rm -f "$work/login.headers" "$work/login.body" "$work/oauth.headers" "$work/finish.headers" "$work/session"
kill "$app_pid" >/dev/null 2>&1 || true
printf 'login token upgrade ok (%s -> %s)\n' "$installed_version" "$target_version"
EOF

printf '%s\n' "$root" > "$work/repo-root"
printf '%s\n' "$tag" > "$work/tag"
printf '%s\n' "$installed_version" > "$work/installed-version"
printf '%s\n' "$issuer" > "$work/issuer"
mkdir -p "$work/etc/kiln"
export KILN_LOGIN_UPGRADE_WORK=$work
sudo -n --preserve-env=KILN_LOGIN_UPGRADE_WORK unshare --mount bash "$script" --root

printf 'login token upgrade checks passed\n'
