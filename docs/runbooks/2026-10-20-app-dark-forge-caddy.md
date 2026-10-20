# Add app.dark-forge.dev to the Caddyfile

Kiln Ops adds the `app.dark-forge.dev` site to `/opt/paseo/Caddyfile` after the release that contains the app units is installed. This pull request does not SSH, does not edit the host, and does not reload Caddy.

Restart `kiln-identity` only after Jamie says yes. This pull request does not run that restart. The browser check is for Kiln Ops after the release. This pull request does not run it.

`deploy/kiln-identity.service` names the unit `kiln-identity`, the database `/var/lib/kiln-identity/identity.db`, and the binary `/usr/local/libexec/kiln-identity`. The new binary sets `user_version` to 2 when it opens that database. The previous binary refuses `user_version` 2. The new binary has no path back to `user_version` 1.

## Copy the dashboard before the installer

`deploy/install.sh` renames `/usr/local/share/kiln/dashboard` aside and then moves the new tree into that path. If that move fails, the installer puts the old tree back and then stops with `cannot install dashboard archive`. The live directory is already back. If putting the old tree back fails, the installer stops with `cannot restore the dashboard`. The old tree stays in the aside, and a later successful install deletes that aside. Copy the live directory before you run the installer. This pull request does not run the installer.

```sh
install -d -m 700 /var/backups/kiln-dashboard-app
if [ -d /usr/local/share/kiln/dashboard ]; then
  rm -rf /var/backups/kiln-dashboard-app/dashboard
  cp -a /usr/local/share/kiln/dashboard /var/backups/kiln-dashboard-app/dashboard
fi
```

Copy the release `kiln-dashboard` archive to `/var/tmp/kiln-dashboard.tar.gz`. From the extracted server archive, run the installer.

```sh
bash install.sh --apply --dashboard-archive /var/tmp/kiln-dashboard.tar.gz
```

If that command stops with `cannot install dashboard archive`, the live tree is already in place. Stop. Leave the backup where it is.

If the installer stops with `cannot restore the dashboard`, put the copy back before you run the installer again. A later successful install deletes the aside.

```sh
rm -rf /usr/local/share/kiln/dashboard
cp -a /var/backups/kiln-dashboard-app/dashboard /usr/local/share/kiln/dashboard
```

Skip the restore when `/usr/local/share/kiln/dashboard` is still there. A failure before the move leaves the live tree in place.

## Start kiln-app

The installer enables `kiln-app.service` and does not start it. Start it now. `127.0.0.1:8093` listening is the state you want after this step.

```sh
systemctl restart kiln-app
systemctl is-active kiln-app
ss -ltn 'sport = :8093'
curl -sS -D /tmp/kiln-app-loopback.headers -o /tmp/kiln-app-loopback.json http://127.0.0.1:8093/v1/x
```

Stop unless `systemctl is-active kiln-app` prints `active`, `ss` shows `127.0.0.1:8093`, the headers contain `404` and `application/json`, and the body contains `"code":"not_found"`.

## Start the bridge socket

`deploy/app.Caddyfile` dials `172.18.0.1:8093`. That address is `kiln-app-proxy.socket`. The installer enables and starts `kiln-app-proxy.socket`. Run these commands to confirm the socket is active. Running the commands again does not stop an active socket. Without the socket, Caddy has no listener and the site returns 502.

```sh
systemctl enable --now kiln-app-proxy.socket
systemctl is-active kiln-app-proxy.socket
ss -ltn 'sport = :8093'
```

Stop unless `systemctl is-active kiln-app-proxy.socket` prints `active` and `ss` shows `172.18.0.1:8093`. The socket allows only `172.18.0.3/32`. Do not use a connection from the host as the check.

## Copy the Caddyfile before the edit

Copy `/opt/paseo/Caddyfile` to a backup beside it. Record both checksums.

```sh
cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-app-dark-forge
sha256sum /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-app-dark-forge
```

Append the `app.dark-forge.dev` site from `deploy/app.Caddyfile` when that site is not already present. If a whole line is `}app.dark-forge.dev {`, split that line before you append. A comment or an indented line stays as it is. If the live file has no trailing newline, add one before you append. A bare `cat >>` joins the previous `}` to `app.dark-forge.dev {` and Caddy rejects the file. Leave every other site as it is. Keep each existing line's ending. A split line keeps that line's ending. A line you append uses the ending at the end of the file. Run this from the extracted server archive.

```sh
fixed=$(mktemp)
missing=0
if [ -s /opt/paseo/Caddyfile ] && [ -n "$(tail -c1 /opt/paseo/Caddyfile)" ]; then
  missing=1
fi
if awk -v missing="$missing" '
  {
    line = $0
    ending = "\n"
    if (sub(/\r$/, "", line)) ending = "\r\n"
    n++
    text[n] = line
    endings[n] = ending
    if (line == "}app.dark-forge.dev {") {
      found = 1
      split_at[n] = 1
    } else if (line == "app.dark-forge.dev {") found = 1
  }
  END {
    if (missing && n > 0) {
      if (endings[n] == "\r\n") endings[n] = "\r"
      else endings[n] = ""
    }
    for (i = 1; i <= n; i++) {
      sep = endings[i]
      if (split_at[i]) {
        if (sep == "") sep = "\n"
        printf "%s%s", "}", sep
        printf "%s%s", "app.dark-forge.dev {", endings[i]
      } else printf "%s%s", text[i], sep
    }
    exit !found
  }
' /opt/paseo/Caddyfile > "$fixed"; then
  cat "$fixed" > /opt/paseo/Caddyfile
else
  cat "$fixed" > /opt/paseo/Caddyfile
  cr=$(printf '\r')
  last=$(tail -c2 /opt/paseo/Caddyfile; printf x)
  use_crlf=0
  case "$last" in
    *"$cr"*) use_crlf=1 ;;
  esac
  if [ -n "$(tail -c1 /opt/paseo/Caddyfile)" ]; then
    printf '\n' >> /opt/paseo/Caddyfile
  fi
  if [ "$use_crlf" -eq 1 ]; then
    awk '{ sub(/\r$/, ""); printf "%s\r\n", $0 }' deploy/app.Caddyfile >> /opt/paseo/Caddyfile
  else
    cat deploy/app.Caddyfile >> /opt/paseo/Caddyfile
  fi
fi
rm -f "$fixed"
```

Running the block again does not append a second site, and it leaves a repaired file unchanged.

Do not repeat the container recreate in `docs/runbooks/2026-10-20-paseo-caddy-override.md`. Leave `COMPOSE_FILE` as it is. Leave the CA mount as it is.

Validate the edited file with the running container. If validate fails, copy `/opt/paseo/Caddyfile.bak-app-dark-forge` back to `/opt/paseo/Caddyfile` and do not reload.

```sh
docker exec caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

## Check DNS before the reload

Reload only after validate exits 0 and `app.dark-forge.dev` and `dark-forge.dev` resolve to the same set of addresses. On 2026-10-20 both names resolve to `78.46.98.73`. The check compares every address from `getent ahostsv4`. It does not pin that number. If the sets differ, either name has no address, or validate exits non-zero, copy the backup back and do not reload.

```sh
app=$(getent ahostsv4 app.dark-forge.dev | awk '{ print $1 }' | sort -u)
apex=$(getent ahostsv4 dark-forge.dev | awk '{ print $1 }' | sort -u)
printf '%s\n' "$app" "$apex"
if [ "$app" != "$apex" ] || [ -z "$app" ] || [ -z "$apex" ]; then
  cp -a /opt/paseo/Caddyfile.bak-app-dark-forge /opt/paseo/Caddyfile
  printf '%s\n' 'DNS does not match. Restored the Caddyfile. Did not reload.'
elif ! docker exec caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile; then
  cp -a /opt/paseo/Caddyfile.bak-app-dark-forge /opt/paseo/Caddyfile
  printf '%s\n' 'Caddyfile failed validation. Restored the Caddyfile. Did not reload.'
else
  docker exec caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile
fi
```

The site proxies to `172.18.0.1:8093` in cleartext. It sets `X-Forwarded-For` from the client address. It does not set `Cache-Control`. The listener keeps its asset cache. Caddy obtains the certificate for `app.dark-forge.dev` with the ACME client it already uses.

## Check the sites after reload

`https://dark-forge.dev/login` stays HTTP 200 and the body contains `Light the way in.` `https://cli.dark-forge.dev/v1/templates` with no credential stays HTTP 401. `https://app.dark-forge.dev/` returns HTTP 200 with `text/html`. `https://app.dark-forge.dev/v1/x` returns HTTP 404 with `application/json` and `"code":"not_found"`. Public tcp/8443 stays closed. Do not add an accept rule.

```sh
curl -sS -D /tmp/kiln-login.headers -o /tmp/kiln-login.body https://dark-forge.dev/login
curl -sS -D /tmp/kiln-cli.headers -o /tmp/kiln-cli.body https://cli.dark-forge.dev/v1/templates
curl -sS -D /tmp/kiln-app.headers -o /tmp/kiln-app.body https://app.dark-forge.dev/
curl -sS -D /tmp/kiln-app-v1x.headers -o /tmp/kiln-app-v1x.json https://app.dark-forge.dev/v1/x
```

If sign-in or `:443` is wrong, or the new site is not that 200 and that JSON 404, copy the backup back. Reload that file only when validate exits 0. When validate fails, leave the running Caddy process untouched and print the failure.

```sh
rollback=0
grep -q 'HTTP/.* 200' /tmp/kiln-login.headers || rollback=1
grep -q 'Light the way in.' /tmp/kiln-login.body || rollback=1
grep -q 'HTTP/.* 401' /tmp/kiln-cli.headers || rollback=1
grep -q 'HTTP/.* 200' /tmp/kiln-app.headers || rollback=1
grep -q 'text/html' /tmp/kiln-app.headers || rollback=1
grep -q 'HTTP/.* 404' /tmp/kiln-app-v1x.headers || rollback=1
grep -q 'application/json' /tmp/kiln-app-v1x.headers || rollback=1
grep -q '"code":"not_found"' /tmp/kiln-app-v1x.json || rollback=1
if [ "$rollback" -eq 1 ]; then
  cp -a /opt/paseo/Caddyfile.bak-app-dark-forge /opt/paseo/Caddyfile
  if docker exec caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile; then
    docker exec caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile
  else
    printf '%s\n' 'Restored Caddyfile failed validation. Did not reload.'
  fi
fi
```

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
