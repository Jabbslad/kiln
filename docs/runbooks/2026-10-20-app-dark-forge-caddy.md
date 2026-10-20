# Add app.dark-forge.dev to the Caddyfile

Kiln Ops adds the `app.dark-forge.dev` site to `/opt/paseo/Caddyfile` after the release that contains the app units is installed. This pull request does not SSH, does not edit the host, and does not reload Caddy.

Restart `kiln-identity` only after Jamie says yes. This pull request does not run that restart. The browser check is for Kiln Ops after the release. This pull request does not run it.

`deploy/kiln-identity.service` names the unit `kiln-identity`, the database `/var/lib/kiln-identity/identity.db`, and the binary `/usr/local/libexec/kiln-identity`. The new binary sets `user_version` to 2 when it opens that database. The previous binary refuses `user_version` 2. The new binary has no path back to `user_version` 1.

## Check port 8093

Do this before you edit `/opt/paseo/Caddyfile`. Port 8093 is free in this repository. It is unproven on the live host.

```sh
ss -ltn 'sport = :8093'
```

Stop if that output shows `127.0.0.1:8093` or `172.18.0.1:8093`. This procedure does not start `kiln-app.service`.

## Copy the Caddyfile before the edit

Copy `/opt/paseo/Caddyfile` to a backup beside it. Record both checksums.

```sh
cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-app-dark-forge
sha256sum /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-app-dark-forge
```

Append only the `app.dark-forge.dev` site from `deploy/app.Caddyfile`. Leave every other site as it is.

```sh
cat deploy/app.Caddyfile >> /opt/paseo/Caddyfile
```

Do not repeat the container recreate in `docs/runbooks/2026-10-20-paseo-caddy-override.md`. Leave `COMPOSE_FILE` as it is. Leave the CA mount as it is.

Validate the edited file with the running container. Reload only after validate exits 0. If validate fails, copy `/opt/paseo/Caddyfile.bak-app-dark-forge` back to `/opt/paseo/Caddyfile` and do not reload.

```sh
docker exec caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
docker exec caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile
```

The site proxies to `172.18.0.1:8093` in cleartext. It sets `X-Forwarded-For` from the client address. It does not set `Cache-Control`. The listener keeps its asset cache. Caddy obtains the certificate for `app.dark-forge.dev` with the ACME client it already uses.

## Check the existing names

After reload, `https://dark-forge.dev/login` stays HTTP 200 and the body contains `Light the way in.` `https://cli.dark-forge.dev/v1/templates` with no credential stays HTTP 401. Public tcp/8443 stays closed. Do not add an accept rule.

`caddy reload` does not restart `kiln-identity`.

## Restart identity only after Jamie says yes

Skip this section until Jamie has approved the identity binary. Stop if `redirect_uri` is still null. This pull request does not run these commands.

```sh
sqlite3 -readonly /var/lib/kiln-identity/identity.db "SELECT redirect_uri FROM servers WHERE active=1;"
```

Stop if that query fails or prints an empty value.

Stop `kiln-identity` before the copy. Copy the database. If `identity.db-wal` exists, copy it. If `identity.db-shm` exists, copy it. Record a checksum of each database file you copy. Copy the current binary. Finish the copies and the checksums before you start the new binary.

```sh
systemctl stop kiln-identity
install -d -m 700 /var/backups/kiln-identity-app
cp -a /var/lib/kiln-identity/identity.db /var/backups/kiln-identity-app/identity.db
if [ -f /var/lib/kiln-identity/identity.db-wal ]; then
  cp -a /var/lib/kiln-identity/identity.db-wal /var/backups/kiln-identity-app/identity.db-wal
fi
if [ -f /var/lib/kiln-identity/identity.db-shm ]; then
  cp -a /var/lib/kiln-identity/identity.db-shm /var/backups/kiln-identity-app/identity.db-shm
fi
sha256sum /var/lib/kiln-identity/identity.db > /var/backups/kiln-identity-app/checksums
if [ -f /var/lib/kiln-identity/identity.db-wal ]; then
  sha256sum /var/lib/kiln-identity/identity.db-wal >> /var/backups/kiln-identity-app/checksums
fi
if [ -f /var/lib/kiln-identity/identity.db-shm ]; then
  sha256sum /var/lib/kiln-identity/identity.db-shm >> /var/backups/kiln-identity-app/checksums
fi
cp -a /usr/local/libexec/kiln-identity /var/backups/kiln-identity-app/kiln-identity
sha256sum /usr/local/libexec/kiln-identity > /var/backups/kiln-identity-app/binary.sha256
install -m 755 bin/kiln-identity /usr/local/libexec/kiln-identity
systemctl start kiln-identity
```

`bin/kiln-identity` is the binary in the identity archive. The `install` replaces `/usr/local/libexec/kiln-identity`. `systemctl start kiln-identity` runs that file. The process sets `user_version` to 2 when it opens the database.

## Restore the previous identity binary

Stop `kiln-identity`. Put the copied database files back. Check the checksums. Start the previous binary. If the backup has no `identity.db-wal`, remove the live `identity.db-wal`. Do the same for `identity.db-shm`. If you leave a newer `identity.db-wal` in place, SQLite replays it onto the restored database.

```sh
systemctl stop kiln-identity
cp -a /var/backups/kiln-identity-app/identity.db /var/lib/kiln-identity/identity.db
if [ -f /var/backups/kiln-identity-app/identity.db-wal ]; then
  cp -a /var/backups/kiln-identity-app/identity.db-wal /var/lib/kiln-identity/identity.db-wal
else
  rm -f /var/lib/kiln-identity/identity.db-wal
fi
if [ -f /var/backups/kiln-identity-app/identity.db-shm ]; then
  cp -a /var/backups/kiln-identity-app/identity.db-shm /var/lib/kiln-identity/identity.db-shm
else
  rm -f /var/lib/kiln-identity/identity.db-shm
fi
sha256sum -c /var/backups/kiln-identity-app/checksums
install -m 755 /var/backups/kiln-identity-app/kiln-identity /usr/local/libexec/kiln-identity
sha256sum -c /var/backups/kiln-identity-app/binary.sha256
systemctl start kiln-identity
```

The last `install` writes the copied binary back to `/usr/local/libexec/kiln-identity`. Do not start the new binary on the restored files. The new binary sets `user_version` to 2 again if it opens the restored files.
