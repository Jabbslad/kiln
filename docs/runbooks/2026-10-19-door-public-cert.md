# Serve cli.dark-forge.dev through Caddy

Kiln Ops puts `https://cli.dark-forge.dev` on port 443. Caddy at `/opt/paseo/Caddyfile` already answers `dark-forge.dev`. It obtains the new name by automatic ACME, the same way it already does for `dark-forge.dev`. The door keeps the private leaf in `/etc/kiln/door.crt` and `/etc/kiln/door.key`. This procedure does not restart `kiln-identity` and does not edit its certificate or key. Do not put a Cloudflare token on the host.

## Get Jamie's yes first

Stop until Jamie says yes at deploy time. This step touches the login service. Do not deploy it from this repository.

The yes covers every touch below.

- Editing `/opt/paseo/Caddyfile` and recreating paseo's `caddy` service with `deploy/paseo-caddy.override.yaml`. That process fronts `https://dark-forge.dev` sign-in.
- Reading `/var/lib/kiln-identity/identity.db` for the server name and the `ca_pem` bytes.
- `PUT https://dark-forge.dev/v1/servers/SERVER_ID` to set `origin` to `https://cli.dark-forge.dev`, with the same `name` and the same `ca_pem`. The call reads `/etc/kiln/directory.token` into a mode 0600 header file. Do not print the token.

## Install the released client first

The door change shipped in the published v0.4.9 release, tag v0.4.9 on ca31d16, and that commit contains the #27 commit 5e9e7da. Kiln 0.4.6 pins the private CA and fails the public certificate. Do not use 0.4.6 for the laptop check. Do not use an unreleased build for that check.

Jamie's saved profile origin is `https://cli.dark-forge.dev:8443`. That file keeps its pinned CA. Door commands do not relax the pin on any port, including 443. `kiln list`, `kiln create`, `kiln exec`, and `kiln delete` fail the public certificate until a fresh `kiln login`.

`kiln login` on a profile that already has a session prints `Already signed in` and does not store the new origin. Run `kiln logout`, then `kiln login`, after the directory origin below is `https://cli.dark-forge.dev`. That login keeps the same CA bytes, so it does not return `server CA changed; explicit re-enrollment is required`. It stores public-root trust for that origin only. `:8443` stays pinned.

## Keep TLS on the upstream hop

`door::serve` loads one certificate and calls `bind_rustls`. The door has no plaintext listener. `kiln-api` already owns `127.0.0.1:8443`, so the door moves to `127.0.0.1:8444` and keeps that private leaf. The door's upstream stays `https://127.0.0.1:8443`.

Caddy runs in Docker at `172.18.0.3`. It cannot dial the host's loopback. The identity bridge is already `172.18.0.1:8092`. Add the same kind of TCP proxy for the door. It listens on `172.18.0.1:8444`, accepts only `172.18.0.3/32`, and forwards to `127.0.0.1:8444`. The bytes on that hop are TLS. The owner JWT stays inside that TLS. Do not bind the door on the bridge.

## One listen address

`kiln-api door --listen` takes one address. `door::serve` binds that address. `deploy/kiln-door.service` passes one `${KILN_DOOR_LISTEN}`. This procedure does not edit those files.

After the rebind the door listens on `127.0.0.1:8444` only. Caddy on 443 terminates the public certificate and dials `https://172.18.0.1:8444`. Do not install a listener on `78.46.98.73:8443`.

The bridge socket allows only `172.18.0.3/32`. That filter would also block laptops if port 8443 were a second listener on the same socket unit. `deploy/kiln-door-proxy.service` names only `kiln-door-proxy.socket`.

Port 443 serves after the rebind. tcp/8443 stays closed. The v0.4.6 record used `78.46.98.73:8443`.

Record `systemctl show kiln-identity.service -p MainPID -p NRestarts -p ActiveEnterTimestamp` before you change anything.

## Install the bridge proxy

From the server archive, install the bridge units. Do not install a public listener.

```
install -m 644 deploy/kiln-door-proxy.service /etc/systemd/system/kiln-door-proxy.service
install -m 644 deploy/kiln-door-proxy.socket /etc/systemd/system/kiln-door-proxy.socket
systemctl daemon-reload
```

## Check DNS and ports 80 and 443

Do this before the recreate. `cli.dark-forge.dev` must be DNS-only and must point at this host. Its A record is the host's public address, the same address `dark-forge.dev` uses. It must not be a Cloudflare proxy address. Leave the record DNS-only. Do not put a Cloudflare token on the host.

Confirm ports 80 and 443 reach the Caddy container. From outside the host, `http://cli.dark-forge.dev/` and `https://dark-forge.dev/login` are answered by that container. Stop if either port does not reach Caddy. Automatic ACME uses those ports.

## Recreate Caddy

Caddy is the `caddy` service of the paseo compose project. Its network is `paseo_default`, its volumes are `paseo_caddy_data` and `paseo_caddy_config`, and the same Caddyfile serves `app.nowcode.com` and `hub.nowcode.com`. Do not start a second Caddy container. `deploy/paseo-caddy.override.yaml` adds `ipv4_address: 172.18.0.3` on the existing `default` network and a read-only bind of `/etc/kiln/ca.crt`. It does not set the image, the ports, the Caddyfile mount, or the volumes.

Caddy requests the certificate for `cli.dark-forge.dev` when the container comes back. There is no `tls` certificate line in the new site. Do not add one.

Copy the live file and record both hashes.

```
cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-door-443
sha256sum /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-door-443
```

Add the site in `deploy/door.Caddyfile`. Leave the `dark-forge.dev` site and the nowcode sites as they are.

```
cli.dark-forge.dev {
	reverse_proxy https://172.18.0.1:8444 {
		transport http {
			versions 1.1
			tls_server_name cli.dark-forge.dev
			tls_trust_pool file {
				pem_file /etc/kiln/ca.crt
			}
		}
	}
}
```

`/etc/kiln/ca.crt` is the path inside the container. The override bind-mounts the host file there with `read_only: true` when Docker creates the container. `docker update` cannot add a mount. A copy placed in the running container disappears on the next recreate.

The bind sets `create_host_path: false`. Create fails when `/etc/kiln/ca.crt` is missing. A short `-v` of a missing path creates a directory, and Caddy then has no certificate file to read.

Do not mount `/etc/kiln`, `/etc/kiln/ca.key`, `/etc/kiln/door.key`, or `/etc/kiln/directory.token`. The certificate is mode 0644. Leave the private keys mode 0600.

The door socket allows only `172.18.0.3/32`. The override pins that address. A recreate that assigns another address no longer matches the allow list, and port 443 stops reaching the door.

## Pre-flight

Do this before anything is stopped. The running caddy container already holds `172.18.0.3`. A check that the address is free would always abort.

```
docker network inspect paseo_default --format '{{json .IPAM.Config}}'
```

Abort unless a `Subnet` in that IPAM Config contains `172.18.0.3`.

```
docker network inspect paseo_default --format '{{json .Containers}}'
```

Abort if any container other than paseo's `caddy` service holds `172.18.0.3`. That service has compose labels `com.docker.compose.project=paseo` and `com.docker.compose.service=caddy`. The running caddy container is allowed to hold the address.

On Docker 29.1.3 with Compose 2.40.3, a compose default network created without an explicit subnet still has an automatic subnet in IPAM Config, and a fixed address is accepted when that subnet contains `172.18.0.3`. `paseo_default` is a user-defined network. The error `user specified IP address is supported on user defined networks only` applies to the built-in `bridge` network, not to `paseo_default`.

If no configured subnet contains `172.18.0.3`, abort the procedure. Do not stop caddy. Docker refuses the pin with `no configured subnet contains IP address 172.18.0.3` and, on this Docker, leaves the existing container running. Replacing `paseo_default` with a new subnet disconnects every container on it, including caddy, paseo, paseo-hub, and paseo-hub-db. That needs a separate plan. Do not create a second bridge on `172.18.0.0/16`.

The running image is `caddy:2`. The override does not change it. On `caddy:2.10.2`, `caddy adapt` accepts `tls_trust_pool file` and warns on `tls_trusted_ca_certs`. Validate the edited Caddyfile with the image the container already uses, and stop if validate fails.

```
docker run --rm \
  --mount type=bind,source=/opt/paseo/Caddyfile,target=/etc/caddy/Caddyfile,readonly \
  --mount type=bind,source=/etc/kiln/ca.crt,target=/etc/kiln/ca.crt,readonly \
  caddy:2 \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

Copy `deploy/paseo-caddy.override.yaml` to `/opt/paseo/paseo-caddy.override.yaml`. In `/opt/paseo/.env`, set `COMPOSE_FILE` to the compose file paseo already uses, a colon, and `paseo-caddy.override.yaml`. If `COMPOSE_FILE` is already set, append `:paseo-caddy.override.yaml` to that value. Do not drop the existing file from the list. A later plain `docker compose up` in `/opt/paseo` keeps the pin and the CA mount.

`paseo_caddy_data` and `paseo_caddy_config` stay the service volumes. The override does not declare new ones. A new volume would make Caddy request the public certificate again.

From `/opt/paseo`, recreate only caddy. Do not copy the certificate into the container. Do not restart `kiln-identity`.

```
docker compose up -d --force-recreate --no-deps caddy
systemctl show kiln-identity.service -p MainPID -p NRestarts -p ActiveEnterTimestamp
```

The container address is `172.18.0.3`. `/etc/kiln/ca.crt` is mounted read-only. The nowcode sites still answer. The identity MainPID and ActiveEnterTimestamp match the values recorded at the start.

Prove the existing sign-in, not only the new name. From outside the host, the body of `https://dark-forge.dev/login` contains `Light the way in.` A non-error status is not enough. On the laptop, with the released client and without `KILN_IDENTITY_CA`, run a login that does not replace the saved default profile.

```
kiln --profile sign-in-check login
```

Approve the device code. The command must exit 0.

If the page is not the sign-in page, or that login does not finish, roll back the Caddy container before any other change.

Stop. Do not rebind the door.

## Roll back the Caddy container

Restore `/opt/paseo/Caddyfile` from `/opt/paseo/Caddyfile.bak-door-443`. Remove `paseo-caddy.override.yaml` from `COMPOSE_FILE` in `/opt/paseo/.env`. The recreated container holds ports 80 and 443 and `172.18.0.3`. Remove it before starting the old config. From `/opt/paseo`:

```
docker compose stop caddy
docker compose rm -f caddy
docker compose up -d --no-deps caddy
```

The old caddy container comes back without the CA mount. `paseo_caddy_data` and `paseo_caddy_config` are the same volumes. Do not restart `kiln-identity`.

## Check the issued certificate

```
openssl s_client -connect cli.dark-forge.dev:443 -servername cli.dark-forge.dev </dev/null 2>/dev/null | openssl x509 -noout -issuer -ext subjectAltName
```

The issuer is a public CA. The SAN includes `DNS:cli.dark-forge.dev`. No Cloudflare token was used.

## Advertise the portless origin

`GET /v1/servers` returns the `origin` column. Store `https://cli.dark-forge.dev`. If the row still names port 8443, change that origin and leave `name` and `ca_pem` byte for byte. A different CA makes `profile::set` return `server CA changed; explicit re-enrollment is required` and keep the old PEM.

Read the row and hash `ca_pem` first. This is the identity database read from the yes above.

```
sqlite3 -readonly /var/lib/kiln-identity/identity.db "SELECT id, name, origin, length(ca_pem) FROM servers WHERE active=1;"
```

Build the JSON with that name, the origin `https://cli.dark-forge.dev`, and the text of `/etc/kiln/ca.crt`. Write a mode 0600 header file the way `deploy/enroll-dark-forge.sh` writes `Authorization: Bearer` plus the contents of `/etc/kiln/directory.token`. `curl` reads the header with `-H @file`. Do not print the token and do not put it in the JSON.

```
curl --fail --silent --show-error --max-time 20 \
  -X PUT \
  -H @/root/kiln-directory.hdr \
  -H 'Content-Type: application/json' \
  --data-binary @/root/kiln-origin.json \
  "https://dark-forge.dev/v1/servers/SERVER_ID"
```

Read the row again. `origin` is `https://cli.dark-forge.dev`. The `ca_pem` hash is unchanged. Delete the header file. Do not run `deploy/enroll-dark-forge.sh` in place of this PUT. The script does not change the directory origin.

## Rebind the door

Do this only after the released client is on the laptop and the directory origin is `https://cli.dark-forge.dev`.

Edit `/etc/kiln/door.env` before stopping the door. The running process keeps the old address until it stops. Leave the installed `kiln-door.service` file as it is.

```
KILN_DOOR_LISTEN=127.0.0.1:8444
```

The unit already passes `--upstream https://127.0.0.1:8443`, `--tls-cert /etc/kiln/door.crt`, and `--tls-key /etc/kiln/door.key`. Stop the door, start the bridge socket, then start the door on loopback. The proxy reads its socket when the process starts, so restart it after the socket is listening and the door is up.

```
systemctl stop kiln-door.service
systemctl enable --now kiln-door-proxy.socket
systemctl start kiln-door.service
systemctl restart kiln-door-proxy.service
```

Port 443 serves after this. Do not enable a listener on tcp/8443.

`ss` shows `127.0.0.1:8444` owned by the door. `172.18.0.1:8444` is the bridge socket. A connection to that bridge address from anywhere other than `172.18.0.3` is refused. `127.0.0.1:8443` stays `kiln-api`. `ss` does not show `78.46.98.73:8443`.

Port 443 is still the public certificate from the check above. `https://cli.dark-forge.dev/v1/templates` with no credential returns 401.

Confirm the identity MainPID and ActiveEnterTimestamp are unchanged.

## Off-host check on port 443

On the laptop, released client, no `KILN_IDENTITY_CA`. There is no `kiln connect` command.

If `kiln login` says `Already signed in`, run `kiln logout` first. Logout revokes that device, then `kiln login` starts a new one. A saved profile that still names port 8443 keeps that pin. It does not move itself to port 443.

```
kiln logout
kiln login
kiln list
kiln create --name door-443
kiln exec door-443 -- /bin/echo kiln-exec-poll
kiln delete door-443
```

`kiln login` talks to `https://dark-forge.dev` and stores public-root trust for `https://cli.dark-forge.dev`. `kiln list`, `kiln create`, `kiln exec`, and `kiln delete` then go through that name on port 443. `kiln exec` polls `GET /v1/operations/{id}` and prints the `stdout` field. Guest output is not a separate byte stream. The dashboard has no websocket.

A missing token, a bogus token, and the administrator token each get HTTP 401 from the door and do not open the dial to `kiln-api`.

If any command on 443 fails, restore the Caddy site from the rollback below. Do not open tcp/8443. A profile that already finished `kiln login` dials port 443, so that laptop stays down until this Caddy site is restored. Leave the firewall alone.

## Close public tcp/8443

Do this only after the laptop login, list, create, exec, and delete succeed on port 443. The rebind leaves `:8443` open on purpose. This step is what closes it.

`deploy/kiln-door-proxy.service` names only `kiln-door-proxy.socket`. A host file that still names `kiln-door-proxy-public.socket` as well starts that listener whenever the service starts, including when that unit is disabled. `systemctl disable --now` removes systemd's fd. The running `systemd-socket-proxyd` keeps the fd it inherited until that process is restarted. A restart done while `Sockets=` still names the public unit listens on `78.46.98.73:8443` again.

`deploy/install.sh` and `deploy/bootstrap/install.sh` do this on upgrade when the public unit is still present. They do not install or enable it. To close it by hand, edit `Sockets=` so it names only `kiln-door-proxy.socket`, reload, stop the public unit, then restart the proxy:

```
systemctl daemon-reload
systemctl disable --now kiln-door-proxy-public.socket
systemctl restart kiln-door-proxy.service
```

If the bridge socket or `kiln-door-proxy.service` is inactive after that, start `kiln-door-proxy.socket` and the service again, and check `https://cli.dark-forge.dev/` at once. `systemctl is-enabled kiln-door-proxy-public.socket` prints `disabled`. `ss` shows `172.18.0.1:8444` and does not show `78.46.98.73:8443`. `https://cli.dark-forge.dev/` with no credential returns 401 through Caddy.

`nftables.service` on this host is disabled and inactive. `nft add rule inet filter input tcp dport 8443 drop` changes only the live ruleset and is not loaded on boot. Do not use that rule as the close. The v0.4.6 `tcp dport 8443 accept` rule can stay. With nothing listening, a connection to `78.46.98.73:8443` is refused. Do not edit Docker nat rules.

The 2026-10-20 close is recorded in `docs/runbooks/2026-10-20-close-public-8443.md`.

## Roll back

Follow Roll back the Caddy container, then validate the restored Caddyfile. The old container is up before any door change. To put the door itself back on the public address, disable `kiln-door-proxy-public.socket` first so `78.46.98.73:8443` is free, write that address back into `/etc/kiln/door.env`, and restart `kiln-door` only. Stop and disable `kiln-door-proxy.socket` as well when the bridge is no longer needed. If the directory origin changed, PUT `https://cli.dark-forge.dev:8443` with the same `name` and the same `ca_pem`. The 2026-10-20 close did not add a drop rule. `nftables.service` is disabled, so a live drop is not how that port stays closed. `kiln-identity` stays up. While the public socket is still forwarding, a 0.4.6 client can keep using `:8443` without this rollback. A profile that already stored public-root trust dials port 443 only, so that laptop stays down until this Caddy site is restored.

### Reopen public tcp/8443

This puts `78.46.98.73:8443` back after the 2026-10-20 close. It is a documented path. It was not run.

The close edited `/etc/systemd/system/kiln-door-proxy.service` and disabled `kiln-door-proxy-public.socket`. Restore the backed-up units from `/var/backups/kiln-close-8443-20261020`, reload, start the public socket, and restart the proxy so it inherits that listener again. Then check `ss` and `curl` against port 443.

```
cp -a /var/backups/kiln-close-8443-20261020/kiln-door-proxy.service /etc/systemd/system/kiln-door-proxy.service
cp -a /var/backups/kiln-close-8443-20261020/kiln-door-proxy.socket /etc/systemd/system/kiln-door-proxy.socket
cp -a /var/backups/kiln-close-8443-20261020/kiln-door-proxy-public.socket /etc/systemd/system/kiln-door-proxy-public.socket
systemctl daemon-reload
systemctl enable --now kiln-door-proxy-public.socket
systemctl restart kiln-door-proxy.service
ss -ltnp | awk 'NR==1 || /:8443/ || /:8444/'
curl -sS -D - -o /dev/null -w 'cli_http:%{http_code}\n' --max-time 20 https://cli.dark-forge.dev/
```

`ss` shows `78.46.98.73:8443` and `172.18.0.1:8444`. The `curl` against `https://cli.dark-forge.dev/` on port 443 returns 401. `kiln-identity` stays up.
