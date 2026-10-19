#!/bin/bash
# Enroll this host with https://dark-forge.dev and open the public door.
# The account is whoever approves the code. Jamie's GitHub login is Jabbslad.
# The identity service is not restarted and its unit is not edited.
set -euo pipefail
umask 077

public_host=cli.dark-forge.dev
identity_origin=https://dark-forge.dev
expected_login=Jabbslad
libexec=/usr/local/libexec/kiln-api
etc=/etc/kiln
db=/var/lib/kiln-identity/identity.db
unit_src=$(dirname "$0")/kiln-door.service

die() {
  printf '%s\n' "$1" >&2
  exit 1
}

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
[ -f "$etc/ca.crt" ] && [ -f "$etc/ca.key" ] || die "missing $etc/ca.crt or $etc/ca.key"
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

if [ ! -f "$etc/door.crt" ] || [ ! -f "$etc/door.key" ]; then
  openssl req -new -newkey rsa:2048 -nodes \
    -subj "/CN=$public_host" \
    -keyout "$etc/door.key" -out "$etc/door.csr" >/dev/null 2>&1
  cat >"$etc/door.ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:$public_host
EOF
  serial=0x$(openssl rand -hex 16)
  openssl x509 -req -in "$etc/door.csr" -CA "$etc/ca.crt" -CAkey "$etc/ca.key" \
    -set_serial "$serial" -days 365 -sha256 -extfile "$etc/door.ext" \
    -out "$etc/door.crt" >/dev/null 2>&1
  rm -f "$etc/door.csr" "$etc/door.ext"
  chown kiln-api:kiln-api "$etc/door.key" "$etc/door.crt"
  chmod 640 "$etc/door.key" "$etc/door.crt"
fi
printf 'KILN_DOOR_LISTEN=%s:8443\n' "$ip" >"$etc/door.env"
chmod 644 "$etc/door.env"
install -m 644 "$unit_src" /etc/systemd/system/kiln-door.service
systemctl daemon-reload
systemctl disable --now kiln-door.service >/dev/null 2>&1 || true

if [ ! -e "$etc/identity.json" ]; then
  printf 'Approve the next code at %s while signed in as GitHub user %s.\n' \
    "$identity_origin" "$expected_login"
  printf 'The page must show that GitHub login. A later rename is a different check.\n'
  "$libexec" enroll \
    --url "https://$public_host:8443" \
    --local-url "https://127.0.0.1:8443" \
    --ca-file "$etc/ca.crt" \
    --name dark-forge
fi

owner=$(sed -n 's/.*"owner_id":"\([^"]*\)".*/\1/p' "$etc/identity.json")
server_id=$(sed -n 's/.*"server_id":"\([^"]*\)".*/\1/p' "$etc/identity.json")
case $owner in
  "" | *[!A-Za-z0-9_-]*) die "unreadable owner_id in $etc/identity.json" ;;
esac
case $server_id in
  "" | *[!A-Za-z0-9_-]*) die "unreadable server_id in $etc/identity.json" ;;
esac
subject=$(sqlite3 -readonly "$db" \
  "SELECT subject FROM provider_identities WHERE account_id='$owner' AND provider='github' AND display_name='$expected_login';")
extra=$(sqlite3 -readonly "$db" \
  "SELECT count(*) FROM provider_identities WHERE account_id='$owner' AND NOT (provider='github' AND display_name='$expected_login');")
if [ -z "$subject" ] || [ "$extra" != 0 ] || [ "$(printf '%s\n' "$subject" | grep -c .)" != 1 ]; then
  printf 'This enrollment is not GitHub user %s. The public door stays off.\n' "$expected_login" >&2
  if [ -f "$etc/directory.token" ]; then
    curl --fail --silent --show-error --max-time 20 \
      -X DELETE \
      -H "Authorization: Bearer $(cat "$etc/directory.token")" \
      "$identity_origin/v1/servers/$server_id" \
      || printf 'Directory deactivation failed. Retry that DELETE before deleting local files.\n' >&2
  fi
  rm -f "$etc/identity.json" "$etc/directory.token" "$etc/enrollment.pending.json" \
    /etc/systemd/system/kiln-api.service.d/identity.conf
  systemctl daemon-reload
  systemctl restart kiln-api.service
  printf 'Removed the enrollment files and restarted kiln-api only. Guest VMs were not deleted.\n' >&2
  printf 'Sign in as GitHub user %s and run this script again.\n' "$expected_login" >&2
  exit 1
fi

printf 'Enrolled owner_id %s is GitHub subject %s display name %s.\n' \
  "$owner" "$subject" "$expected_login"
systemctl enable --now kiln-door.service
code=$(curl --silent --show-error --cacert "$etc/ca.crt" \
  --resolve "$public_host:8443:$ip" --output /dev/null --write-out '%{http_code}' \
  "https://$public_host:8443/v1/templates" || true)
[ "$code" = "401" ] || die "unauthenticated probe returned ${code:-none}"
printf 'Unauthenticated https://%s:8443/v1/templates returned 401.\n' "$public_host"
printf 'kiln-identity.service was not changed.\n'
printf 'If tcp/8443 is filtered, allow it. This script does not change the firewall.\n'
printf 'An nft example is: nft add rule inet filter input tcp dport 8443 accept\n'
printf 'On the laptop, install the client, then run kiln login, kiln list, kiln create, kiln exec, and kiln delete.\n'
printf 'Do not set KILN_IDENTITY_CA. SSH is not served by the public door.\n'
