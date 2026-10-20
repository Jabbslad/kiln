# Serve cli.dark-forge.dev through Caddy

Kiln Ops puts `https://cli.dark-forge.dev` on port 443. Caddy at `/opt/paseo/Caddyfile` already answers `dark-forge.dev`. It obtains the new name by automatic ACME, the same way it already does for `dark-forge.dev`. The door keeps the private leaf in `/etc/kiln/door.crt` and `/etc/kiln/door.key`. This procedure does not restart `kiln-identity` and does not edit its certificate or key. Do not put a Cloudflare token on the host.

## Get Jamie's yes first

Stop until Jamie says yes at deploy time. This step touches the login service. Do not deploy it from this repository.

The yes covers every touch below.

- Editing `/opt/paseo/Caddyfile` and reloading Caddy. That process fronts `https://dark-forge.dev` sign-in.
- Reading `/var/lib/kiln-identity/identity.db` for the server name and the `ca_pem` bytes.
- `PUT https://dark-forge.dev/v1/servers/SERVER_ID` to set `origin` to `https://cli.dark-forge.dev`, with the same `name` and the same `ca_pem`. The call reads `/etc/kiln/directory.token` into a mode 0600 header file. Do not print the token.

## Install the released client first

Install the first release cut from a tree that contains this commit. `Cargo.toml` field `workspace.package.version` is `v0.4.9`. This commit does not change that field. No release tag contains the commit yet. Kiln 0.4.6 pins the private CA and fails the public certificate. Do not use 0.4.6 for the laptop check. Do not use an unreleased build for that check.

Jamie's saved profile origin is `https://cli.dark-forge.dev:8443`. That file keeps its pinned CA. Door commands do not relax the pin on any port, including 443. `kiln list`, `kiln create`, `kiln exec`, and `kiln delete` fail the public certificate until a fresh `kiln login`.

`kiln login` on a profile that already has a session prints `Already signed in` and does not store the new origin. Run `kiln logout`, then `kiln login`, after the directory origin below is `https://cli.dark-forge.dev`. That login keeps the same CA bytes, so it does not return `server CA changed; explicit re-enrollment is required`. It stores public-root trust for that origin only. `:8443` stays pinned.

## Keep TLS on the upstream hop

`door::serve` loads one certificate and calls `bind_rustls`. The door has no plaintext listener. `kiln-api` already owns `127.0.0.1:8443`, so the door moves to `127.0.0.1:8444` and keeps that private leaf. The door's upstream stays `https://127.0.0.1:8443`.

Caddy runs in Docker at `172.18.0.3`. It cannot dial the host's loopback. The identity bridge is already `172.18.0.1:8092`. Add the same kind of TCP proxy for the door. It listens on `172.18.0.1:8444`, accepts only `172.18.0.3/32`, and forwards to `127.0.0.1:8444`. The bytes on that hop are TLS. The owner JWT stays inside that TLS. Do not bind the door on the bridge.

## One listen address

`kiln-api door --listen` takes one address. `door::serve` binds that address. `deploy/kiln-door.service` passes one `${KILN_DOOR_LISTEN}`. This procedure does not edit those files.

After the rebind the door listens on `127.0.0.1:8444` only. `kiln-door-proxy-public.socket` listens on `78.46.98.73:8443` and the same proxy forwards those TCP bytes to `127.0.0.1:8444`. The client on `:8443` sees the door's existing private leaf, so a saved pin keeps working. Caddy on 443 still terminates the public certificate and dials `https://172.18.0.1:8444`.

The bridge socket allows only `172.18.0.3/32`. That filter would also block laptops if `:8443` were a second listener on the same socket unit. The public listener is its own socket, `Service=kiln-door-proxy.service`, with `IPAddressAllow=0.0.0.0/0`.

Both ports keep serving after the rebind. Closing `:8443` is a later step, after the released-client check on 443. Leave the firewall accept rule in place until then. The v0.4.6 record used `78.46.98.73:8443`.

Record `systemctl show kiln-identity.service -p MainPID -p NRestarts -p ActiveEnterTimestamp` before you change anything.

## Install the bridge proxy

From the server archive, install the units. Do not start them yet. The door still holds `78.46.98.73:8443`, and starting the public socket now fails with the address in use.

```
install -m 644 deploy/kiln-door-proxy.service /etc/systemd/system/kiln-door-proxy.service
install -m 644 deploy/kiln-door-proxy.socket /etc/systemd/system/kiln-door-proxy.socket
install -m 644 deploy/kiln-door-proxy-public.socket /etc/systemd/system/kiln-door-proxy-public.socket
systemctl daemon-reload
```

## Check DNS and ports 80 and 443

Do this before the reload. `cli.dark-forge.dev` must be DNS-only and must point at this host. Its A record is the host's public address, the same address `dark-forge.dev` uses. It must not be a Cloudflare proxy address. Leave the record DNS-only. Do not put a Cloudflare token on the host.

Confirm ports 80 and 443 reach the Caddy container. From outside the host, `http://cli.dark-forge.dev/` and `https://dark-forge.dev/login` are answered by that container. Stop if either port does not reach Caddy. Automatic ACME uses those ports.

## Reload Caddy

Caddy requests the certificate for `cli.dark-forge.dev` on this reload. There is no `tls` certificate line in the new site. Do not add one.

Copy the live file and record both hashes.

```
cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-door-443
sha256sum /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-door-443
```

Add this site block. Leave the `dark-forge.dev` site as it is.

```
cli.dark-forge.dev {
	reverse_proxy https://172.18.0.1:8444 {
		transport http {
			tls_server_name cli.dark-forge.dev
			tls_trusted_ca_certs /etc/kiln/ca.crt
		}
	}
}
```

`/etc/kiln/ca.crt` is the path Caddy uses inside the container. The container has to see the host file `/etc/kiln/ca.crt` at that path. Mount that one file read-only if it is not already visible. Do not mount `/etc/kiln/ca.key`, `/etc/kiln/door.key`, or `/etc/kiln/directory.token`.

Find the container whose mounts include `/opt/paseo/Caddyfile`. Read the config path from that container's command or from the mount destination. Validate, and stop if validate fails. Do not reload a file that fails validate.

```
docker exec CONTAINER caddy validate --config CONFIG
```

Reload. Do not restart the container. Do not restart `kiln-identity`.

```
docker exec CONTAINER caddy reload --config CONFIG
systemctl show kiln-identity.service -p MainPID -p NRestarts -p ActiveEnterTimestamp
```

The identity MainPID and ActiveEnterTimestamp match the values recorded at the start.

Prove the existing sign-in, not only the new name. From outside the host, the body of `https://dark-forge.dev/login` contains `Light the way in.` A non-error status is not enough. On the laptop, with the released client and without `KILN_IDENTITY_CA`, run a login that does not replace the saved default profile.

```
kiln --profile sign-in-check login
```

Approve the device code. The command must exit 0.

If the page is not the sign-in page, or that login does not finish, restore the backup before any other change.

```
cp -a /opt/paseo/Caddyfile.bak-door-443 /opt/paseo/Caddyfile
docker exec CONTAINER caddy validate --config CONFIG
docker exec CONTAINER caddy reload --config CONFIG
```

Stop. Do not rebind the door.

## Check the issued certificate

```
openssl s_client -connect cli.dark-forge.dev:443 -servername cli.dark-forge.dev </dev/null 2>/dev/null | openssl x509 -noout -issuer -ext subjectAltName
```

The issuer is a public CA. The SAN includes `DNS:cli.dark-forge.dev`. No Cloudflare token was used.

## Advertise the portless origin

`GET /v1/servers` returns the `origin` column. The enrolled row is `https://cli.dark-forge.dev:8443`. Change that origin and leave `name` and `ca_pem` byte for byte. A different CA makes `profile::set` return `server CA changed; explicit re-enrollment is required` and keep the old PEM.

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

Read the row again. `origin` is `https://cli.dark-forge.dev`. The `ca_pem` hash is unchanged. Delete the header file. Do not run `deploy/enroll-dark-forge.sh`. Its enable path writes `KILN_DOOR_LISTEN` back to the public address.

## Rebind the door

Do this only after the released client is on the laptop and the directory origin is `https://cli.dark-forge.dev`.

Edit `/etc/kiln/door.env` before stopping the door. The running process keeps the old address until it stops. Leave the installed `kiln-door.service` file as it is.

```
KILN_DOOR_LISTEN=127.0.0.1:8444
```

The unit already passes `--upstream https://127.0.0.1:8443`, `--tls-cert /etc/kiln/door.crt`, and `--tls-key /etc/kiln/door.key`. Stop the door, start both proxy sockets, then start the door on loopback. The proxy reads its sockets when the process starts, so restart it after both sockets are listening and the door is up.

```
systemctl stop kiln-door.service
systemctl enable --now kiln-door-proxy.socket kiln-door-proxy-public.socket
systemctl start kiln-door.service
systemctl restart kiln-door-proxy.service
```

`:8443` and `:443` both serve after this. Do not disable the public socket here, and do not add the firewall drop here.

`ss` shows `127.0.0.1:8444` owned by the door and `78.46.98.73:8443` owned by systemd. `172.18.0.1:8444` is the bridge socket. A connection to that bridge address from anywhere other than `172.18.0.3` is refused. `127.0.0.1:8443` stays `kiln-api`.

The `:8443` certificate is the door leaf. The fingerprints match.

```
openssl s_client -connect 78.46.98.73:8443 -servername cli.dark-forge.dev </dev/null 2>/dev/null | openssl x509 -noout -fingerprint -sha256
openssl x509 -in /etc/kiln/door.crt -noout -fingerprint -sha256
```

Port 443 is still the public certificate from the check above. `https://cli.dark-forge.dev/v1/templates` with no credential returns 401. A client that still pins `/etc/kiln/ca.crt` can open `https://cli.dark-forge.dev:8443/v1/templates` and get 401, which means the handshake finished.

Confirm the identity MainPID and ActiveEnterTimestamp are unchanged.

## Off-host check on port 443

On the laptop, released client, no `KILN_IDENTITY_CA`. There is no `kiln connect` command.

If `kiln login` says `Already signed in`, run `kiln logout` first. Logout revokes that device, then `kiln login` starts a new one. The saved `:8443` profile keeps working on `:8443`. It does not move itself to port 443.

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

If any command on 443 fails, leave the public socket up. `:8443` still reaches the door leaf, so a profile that pins the private CA, including a 0.4.6 client, keeps working. A profile that already finished `kiln login` dials port 443, so that laptop stays down until this Caddy site is restored. Leave the firewall alone.

## Close public tcp/8443

Do this only after the laptop login, list, create, exec, and delete succeed. The rebind leaves `:8443` open on purpose. This step is what closes it.

Stop the public socket, then restart the proxy so the process drops that listening socket. The bridge socket stays up for Caddy.

```
systemctl disable --now kiln-door-proxy-public.socket
systemctl restart kiln-door-proxy.service
```

The v0.4.6 record added `tcp dport 8443 accept` on `inet filter` `input`, and that chain's policy is accept. Deleting the accept rule leaves the port open to a later bind. Add a drop rule, then delete the accept rule by the handle from the list. Do not edit Docker nat rules.

`nft add rule` changes the live ruleset only. This host has no `ufw`. A reboot drops the new rule unless a ruleset loaded at boot contains it. Before you treat the drop as permanent, check whether `nftables.service` is enabled and which file it loads.

```
nft list table inet filter
nft add rule inet filter input tcp dport 8443 drop
```

From outside the host, a connection to port 8443 fails. `https://cli.dark-forge.dev/v1/templates` with no credential returns 401.

## Roll back

Restore `/opt/paseo/Caddyfile` from `/opt/paseo/Caddyfile.bak-door-443`, validate, and reload. To put the door itself back on the public address, disable `kiln-door-proxy-public.socket` first so `78.46.98.73:8443` is free, write that address back into `/etc/kiln/door.env`, and restart `kiln-door` only. Stop and disable `kiln-door-proxy.socket` as well when the bridge is no longer needed. If the directory origin changed, PUT `https://cli.dark-forge.dev:8443` with the same `name` and the same `ca_pem`. Leave the drop rule out if you have not closed 8443 yet. `kiln-identity` stays up. While the public socket is still forwarding, a 0.4.6 client can keep using `:8443` without this rollback. A profile that already stored public-root trust dials port 443 only, so that laptop stays down until this Caddy site is restored.
