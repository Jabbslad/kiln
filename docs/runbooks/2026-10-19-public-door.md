# Public door on dark-forge

Laptops reach this host at `https://cli.dark-forge.dev:8443`. The door checks an identity JWT, then dials loopback `kiln-api`. A request without that JWT does not open that dial. The administrator token stays on loopback. `https://dark-forge.dev` stays the identity service.

`kiln-identity.service` is not edited and is not restarted. The identity crate did change. Release builds of `kiln-identity` accept only the production GitHub endpoints (`https://github.com/login/oauth/authorize`, `https://github.com/login/oauth/access_token`, and `https://api.github.com/user`) and no extra provider CA. The non-default Cargo feature `lab-provider` puts `--github-authorize-url`, `--github-token-url`, `--github-user-url`, and `--provider-ca-file` back for a lab issuer. Release archives do not enable that feature. `https://dark-forge.dev` refuses a substitute provider even when the feature is on.

SSH is not on the public door. Direct administrator recovery stays `laptop.tar.gz` on the host.

## Before you start

The installed `kiln-api` must be a build that contains the `door` command. `kiln-api` must still be listening on `127.0.0.1:8443`. Do not bind it on `0.0.0.0`. That address takes the loopback port as well.

Create a DNS A record for `cli.dark-forge.dev` to this host's public IPv4 address. The door binds that address, not every address.

The approving account is Jamie's GitHub login `Jabbslad`, numeric id `3084`. Both have to match. A renamed login or a different numeric id fails closed.

## Operator command

From the extracted server archive, as root, pass the public IPv4 address:

```sh
sudo ./deploy/enroll-dark-forge.sh PUBLIC_IPV4
```

The script checks that the address is assigned on this host and that `cli.dark-forge.dev` resolves to it. It does not issue a certificate, install the unit, or change `kiln-door.service` until the enrollment check below passes.

If `/etc/kiln/identity.json`, `/etc/kiln/directory.token`, `/etc/kiln/enrollment.pending.json`, or `/etc/systemd/system/kiln-api.service.d/identity.conf` already exists, the script does not enroll and does not delete those files. It reads the stored server URL from `servers.origin` and continues only when that URL is exactly `https://cli.dark-forge.dev:8443` and the account check below passes. Any other result exits and leaves the files and `kiln-door.service` as they were.

If none of those files exist, it runs:

```sh
sudo /usr/local/libexec/kiln-api enroll \
  --url https://cli.dark-forge.dev:8443 \
  --local-url https://127.0.0.1:8443 \
  --ca-file /etc/kiln/ca.crt \
  --name dark-forge
```

`--local-url` is the readiness dial. Readiness uses the administrator token on loopback. The public door does not accept that token. Approve the printed code at `https://dark-forge.dev` while the browser shows GitHub user `Jabbslad` (numeric id `3084`).

The script then reads `/var/lib/kiln-identity/identity.db`. It continues only when `servers.origin` for the enrolled server is exactly `https://cli.dark-forge.dev:8443` and the enrolled `owner_id` has one provider row: `provider=github`, `display_name=Jabbslad`, and `subject=3084`. It prints that subject.

On a match it issues a leaf from `/etc/kiln/ca.crt` with SAN `DNS:cli.dark-forge.dev` when those files are absent, installs `kiln-door.service`, starts it, and probes `https://cli.dark-forge.dev:8443/v1/templates` without a credential. The probe must be HTTP 401.

On a mismatch of an enrollment this run just created, it deactivates that server at the identity service. The directory token is passed to `curl` in a mode 0600 header file (`-H @file`), not on the command line. It then removes only `identity.json`, `directory.token`, `enrollment.pending.json`, and the API identity drop-in created by this run, and restarts `kiln-api` only. It does not delete guest state and it does not enable the door. Sign in as `Jabbslad` and run the script again. A failed `enroll` command leaves its pending receipt so `--resume` still works.

The script does not change the firewall. If tcp/8443 is filtered, allow it. An nft example, only where nft is already the filter, is `nft add rule inet filter input tcp dport 8443 accept`.

## Laptop

Install the client, then:

```sh
kiln login
kiln list
kiln create
kiln exec BOX -- /bin/echo hello
kiln delete BOX
```

Do not set `KILN_IDENTITY_CA`. That variable is refused for `https://dark-forge.dev`.
