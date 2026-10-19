# Public door on dark-forge

Laptops reach this host at `https://cli.dark-forge.dev:8443`. The door checks an identity JWT, then dials loopback `kiln-api`. A request without that JWT does not open that dial. The administrator token stays on loopback. `https://dark-forge.dev` stays the identity service. This runbook does not change `kiln-identity.service`.

SSH is not on the public door. Direct administrator recovery stays `laptop.tar.gz` on the host.

## Before you start

The installed `kiln-api` must be a build that contains the `door` command. `kiln-api` must still be listening on `127.0.0.1:8443`. Do not bind it on `0.0.0.0`. That address takes the loopback port as well.

Create a DNS A record for `cli.dark-forge.dev` to this host's public IPv4 address. The door binds that address, not every address.

The approving account is Jamie's GitHub login, `Jabbslad`. GitHub can rename a login later. The check below records the login shown at approval, and the numeric GitHub subject.

## Operator command

From the extracted server archive, as root, pass the public IPv4 address:

```sh
sudo ./deploy/enroll-dark-forge.sh PUBLIC_IPV4
```

The script checks that the address is assigned on this host and that `cli.dark-forge.dev` resolves to it. It issues a leaf from `/etc/kiln/ca.crt` with SAN `DNS:cli.dark-forge.dev`, installs `kiln-door.service`, and leaves that unit stopped.

If `/etc/kiln/identity.json` is absent, it runs:

```sh
sudo /usr/local/libexec/kiln-api enroll \
  --url https://cli.dark-forge.dev:8443 \
  --local-url https://127.0.0.1:8443 \
  --ca-file /etc/kiln/ca.crt \
  --name dark-forge
```

`--local-url` is the readiness dial. Readiness uses the administrator token on loopback. The public door does not accept that token. Approve the printed code at `https://dark-forge.dev` while the browser shows GitHub user `Jabbslad`.

The script then reads `/var/lib/kiln-identity/identity.db`. It continues only when the enrolled `owner_id` has one provider row, `provider=github` and `display_name=Jabbslad`. It prints that row's numeric `subject`.

On a match it enables `kiln-door.service` and probes `https://cli.dark-forge.dev:8443/v1/templates` without a credential. The probe must be HTTP 401.

On a mismatch it deactivates the server at the identity service with `/etc/kiln/directory.token`, removes `identity.json`, `directory.token`, and the API identity drop-in, and restarts `kiln-api` only. It does not delete guest state and it does not enable the door. Sign in as `Jabbslad` and run the script again.

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
