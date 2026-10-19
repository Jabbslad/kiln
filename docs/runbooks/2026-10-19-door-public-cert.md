# Serve cli.dark-forge.dev through Caddy

Kiln Ops puts `https://cli.dark-forge.dev` on port 443. The existing Caddy at `/opt/paseo/Caddyfile` presents the public certificate. The door keeps the private leaf in `/etc/kiln/door.crt` and `/etc/kiln/door.key`. This procedure does not restart `kiln-identity`, does not edit its certificate or key, and does not issue a certificate.

## Install the client first

Install a `kiln` binary built from the commit that dials this name with public roots. The published 0.4.6 client pins `/etc/kiln/ca.crt` and fails that handshake. `Cargo.toml` field `workspace.package.version` is `0.4.7`. This commit does not change that field. No release tag contains the commit yet. The first release cut from a tree that contains it is the client required before the swap. Do not use 0.4.6 for the laptop test below.

## Keep TLS on the upstream hop

`door::serve` loads one certificate and calls `bind_rustls`. The door has no plaintext listener. `kiln-api` already owns `127.0.0.1:8443`, so the door moves to `127.0.0.1:8444` and keeps that private leaf. The door's upstream stays `https://127.0.0.1:8443`.

Caddy runs in Docker at `172.18.0.3`. It cannot dial the host's loopback. The identity bridge is already `172.18.0.1:8092`. Add the same kind of TCP proxy for the door. It listens on `172.18.0.1:8444`, accepts only `172.18.0.3/32`, and forwards to `127.0.0.1:8444`. The bytes on that hop are TLS. The owner JWT stays inside that TLS. Do not bind the door on the bridge.

## Bind the door on loopback

Record `systemctl show kiln-identity.service -p MainPID -p NRestarts -p ActiveEnterTimestamp` before you change anything.

Edit `/etc/kiln/door.env`. Leave the installed `kiln-door.service` file as it is. Set the listen line to the loopback port.

```
KILN_DOOR_LISTEN=127.0.0.1:8444
```

The unit already passes `--upstream https://127.0.0.1:8443`, `--tls-cert /etc/kiln/door.crt`, and `--tls-key /etc/kiln/door.key`. Restart the door only.

```
systemctl restart kiln-door.service
```

Confirm the new process listens on `127.0.0.1:8444`. Confirm the identity MainPID and ActiveEnterTimestamp are unchanged. Public port 8443 stops answering when the door leaves the public address. Leave the firewall rule in place until the laptop test passes, so a rollback can bind the public address again without editing nftables first.

## Install the bridge proxy

From the server archive, install the two units and start the socket.

```
install -m 644 deploy/kiln-door-proxy.service /etc/systemd/system/kiln-door-proxy.service
install -m 644 deploy/kiln-door-proxy.socket /etc/systemd/system/kiln-door-proxy.socket
systemctl daemon-reload
systemctl enable --now kiln-door-proxy.socket
```

`ss` shows `172.18.0.1:8444` owned by systemd. A connection from anywhere other than `172.18.0.3` is refused by the socket unit.

## Add the Caddy site

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

Find the container whose mounts include `/opt/paseo/Caddyfile`. Read the config path from that container's command or from the mount destination. Validate, and stop if validate fails.

```
docker exec CONTAINER caddy validate --config CONFIG
```

Reload. Do not restart the container. Do not restart `kiln-identity`.

```
docker exec CONTAINER caddy reload --config CONFIG
systemctl show kiln-identity.service -p MainPID -p NRestarts -p ActiveEnterTimestamp
```

The identity MainPID and ActiveEnterTimestamp match the values recorded at the start.

## Advertise the portless origin

`GET /v1/servers` returns the `origin` column. The enrolled row is `https://cli.dark-forge.dev:8443`. Change that origin and leave `name` and `ca_pem` byte for byte. A different CA makes `profile::set` return `server CA changed; explicit re-enrollment is required` and keep the old PEM.

Read the row and hash `ca_pem` first.

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

## Check login, then the laptop

From the host, `curl --silent --show-error --max-time 15 -o /dev/null -w '%{http_code}\n' https://dark-forge.dev/login` returns a non-error status, and the identity MainPID is still the one you recorded.

On the laptop, use the client from this commit. Do not set `KILN_IDENTITY_CA`. `kiln login` talks to `https://dark-forge.dev`. Create, exec, and delete then go through `https://cli.dark-forge.dev` with no port in the URL. There is no `kiln connect` command.

`kiln exec` polls `GET /v1/operations/{id}` and reads one JSON body. Guest output is the `stdout` field on the finished operation. The dashboard has no websocket. The browser trusts the public certificate for that origin. The page does not read `ca.crt`.

A missing token, a bogus token, and the administrator token each get HTTP 401 from the door and do not open the dial to `kiln-api`.

## Close public tcp/8443

Do this only after the laptop create, exec, and delete succeed.

The v0.4.6 record added `tcp dport 8443 accept` on `inet filter` `input`, and that chain's policy is accept. Deleting the accept rule leaves the port open. Add a drop rule, then delete the accept rule by the handle from the list. Do not edit Docker nat rules.

```
nft list table inet filter
nft add rule inet filter input tcp dport 8443 drop
```

From outside the host, a connection to port 8443 fails. `https://cli.dark-forge.dev/v1/templates` with no credential returns 401.

## Roll back

Restore `/opt/paseo/Caddyfile` from `/opt/paseo/Caddyfile.bak-door-443`, validate, and reload. Put the previous public address back in `/etc/kiln/door.env`. The v0.4.6 record used `78.46.98.73:8443`. Restart `kiln-door` only. Stop and disable `kiln-door-proxy.socket`. If the directory origin changed, PUT `https://cli.dark-forge.dev:8443` with the same `name` and the same `ca_pem`. Leave the drop rule out if you have not closed 8443 yet. `kiln-identity` stays up. The 0.4.6 client can use port 8443 again. The client from this commit dials port 443 only, so that laptop stays down until this Caddy site is restored.
