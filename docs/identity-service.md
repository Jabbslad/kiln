# Kiln identity service

Central login is under development. No production identity hostname is configured
in the public CLI. Real GitHub/Google consent and enrolled-host acceptance must
pass before this can be advertised as ready. Installing the server does not
install this service, register OAuth applications, or enroll a host.

The identity process has its own SQLite database and no KVM/runtime dependency.
It stores provider subjects, device credentials, registered endpoints and public
CA certificates. VM operations, SSH and SFTP go directly from the laptop to the
private server. The laptop still needs LAN/VPN connectivity.

## Provisioning requires explicit operator approval

Choose a public DNS name and HTTPS reverse proxy. Register a GitHub OAuth app and
a Google web OAuth client with these exact callback paths at that origin:

- `/oauth/github/callback`
- `/oauth/google/callback`

Create an unprivileged `kiln-identity` service account. Install the standalone
binary at `/usr/local/libexec/kiln-identity`. Its release package contains only
that binary, the example environment file, service unit, this guide, and version
metadata—not VM images or credentials.

Use `deploy/identity.example.env` as `/etc/kiln-identity/service.env`, replacing
the example origin and public client IDs. Supply provider secrets in separate
files named `github.secret` and `google.secret`, owned by `kiln-identity`, mode
0600, inside an owner-only `/etc/kiln-identity` directory. Never put secrets in
environment variables, shell arguments, repositories or logs.

Generate a unique P-256 PKCS#8 signing key in `signing.pem` using
`openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out signing.pem`
inside that private directory with `umask 077`. Set a unique public signing key
ID in the environment file. Back up the signing key separately from the database.

The supplied systemd unit listens only on loopback port 8091. Terminate HTTPS at
the approved reverse proxy, overwrite `X-Forwarded-For` with the direct client
address, and proxy to loopback. Never preserve a visitor-supplied forwarded
address. Disable access-log query strings and redact Authorization/Cookie headers;
OAuth callback URLs contain one-use codes. Enforce request/connection limits at
the public proxy as well. `--trusted-proxy-loopback` is only appropriate for this
explicitly configured proxy. The app does not manage public DNS or certificates.

## Enroll a self-hosted server

On an installed server, run as its local administrator:

```sh
sudo /usr/local/libexec/kiln-api enroll \
  --issuer https://YOUR-IDENTITY-ORIGIN \
  --url https://YOUR-PRIVATE-SERVER:8443 --ca-file /etc/kiln/ca.crt
```

The terminal prints a public approval code/URL. Sign in using GitHub or Google
and confirm the server endpoint and certificate fingerprint. The registration
secret stays on the host. Signing in alone never approves a pending request.

Enrollment saves root-owned `/etc/kiln/identity.json` (0640, group `kiln-api`) and
root-only `directory.token` (0600). It adds an API-only StateDirectory drop-in,
restarts **only the API**, checks HTTPS readiness with the independent admin
credential, then activates discovery. API and SSH connections briefly disconnect;
guest processes are not restarted. Existing enrollment is never silently replaced.

If readiness or activation fails after persistence, retain the private receipt
and run `sudo /usr/local/libexec/kiln-api enroll --resume`. An ambiguous final
registration response before persistence requires a fresh enrollment; an inactive
registration cannot issue VM tokens and expires after one hour. Do not delete VM
state or rerun the installer as an enrollment recovery procedure.

## Laptop login and recovery

For a development deployment, explicitly set `KILN_IDENTITY_URL` or use
`kiln --issuer https://YOUR-IDENTITY-ORIGIN login`. A verified public origin will
be compiled into release clients only after deployment acceptance. First-use
interactive VM commands can start login; scripts, JSON mode, SSH proxy processes
and explicit missing profiles never open browsers automatically.

The browser URL and public code are printed to stderr if browser launch fails.
One server is selected automatically; multiple servers prompt for a selection.
`kiln login --server SERVER_ID` selects explicitly. Use a separate `--profile`
for an existing direct administrator profile. A changed issuer or CA is rejected,
not accepted silently. Keep the admin recovery profile and `laptop.tar.gz` private.

Use `kiln auth status`, `kiln auth devices list`, and
`kiln auth devices revoke DEVICE_ID` to inspect/revoke sessions. `kiln logout`
attempts remote revocation before removing local credentials and reports if remote
revocation was not confirmed. It does not remove direct administrator profiles.

Create a noninteractive credential with
`kiln auth keys create --server SERVER_ID --scope read --output /PRIVATE/key`.
Then configure a separate automation profile using
`kiln --profile ci --issuer https://YOUR-IDENTITY-ORIGIN login --auth-token-file /PRIVATE/key`.
The original key remains in its private file and is exchanged for short-lived
server tokens. `read` permits inventory only, never SSH or stored operation/exec
output. `operate` permits VM operations but cannot manage accounts or credentials.
Keys are bound to one server, default to 90-day expiry, and can be revoked through
`kiln auth keys revoke KEY_ID`.

## Revocation, outages and key rotation

Device requests expire after ten minutes and start at a five-second poll interval.
Device refresh secrets rotate, expire after 30 days idle or 90 days absolute, and
spent-secret reuse revokes the family. Clients serialize renewal using a private
cross-process lock. A crash or ambiguous exchange invalidates the local session
instead of replaying a potentially spent secret.

Server ES256 tokens last five minutes with at most 30 seconds of clock skew.
Central revocation prevents new tokens immediately; already issued server tokens
can remain usable for that bounded interval. Existing SSH streams and guests do
not depend on continued identity-service availability. Cached server tokens work
offline until renewal is needed. Independent administrator tokens still work.

For signing-key rotation, configure a new private key and unique key ID and supply
the old **public** JWKS with `--retiring-jwks-file`. The file must be owner-only;
duplicate key IDs and private parameters are rejected. Keep old verification keys
published for at least 330 seconds after last issuance. Gateways bind their
atomic `/var/lib/kiln-api/jwks.json` cache to the enrolled issuer. Unknown-key fetches
are limited to once per minute. Emergency cache removal and API restart require
explicit local administration; ordinary key rotation does not disable TLS checks.

Back up SQLite with its online backup API, or stop only the identity service
before copying the database together with its WAL. Encrypt backups: browser OAuth
attempts briefly contain PKCE secrets, and all account/runtime metadata is private.
Restore into an owner-only directory and verify file ownership before starting.
There is no released schema migration tool; back up before software changes and
never point development builds at production identity state.

GitHub and Google subjects are distinct identities even when emails match.
Cross-provider account linking, teams, invitations and hosted tenant isolation
are not implemented. Windows central credential storage uses a protected ACL
allowing only the current user and LocalSystem; native execution awaits CI.
Unix storage enforces owner-only files/directories. Unsafe existing ACLs or
permissions are rejected rather than silently modified.

## Verification and remaining acceptance

Deterministic tests exercise real OAuth/OIDC libraries against local HTTPS
fixtures, signed JWT validation, refresh replay, ownership and scope boundaries,
credential persistence, and disposable enrollment failure/resume paths. Browser
checks exercise real rendered forms, keyboard focus, escaping and mobile layout.
The pages use `Referrer-Policy: strict-origin`: `no-referrer` makes native form
POSTs send `Origin: null`, breaking the explicit origin check. Neither policy
change nor testing disables CSRF or TLS validation.

These checks do not establish real GitHub/Google consent or central-authenticated
VM access. After approved provider setup and disposable host enrollment,
`scripts/test-central-login.py --config PRIVATE_PROFILE --box DISPOSABLE_BOX_ID`
checks real central status, inventory, binary exec and SSH without changing host
configuration. It has not yet run against a deployed central service. SFTP/PTY,
multi-process renewal, revocation and identity-outage acceptance remain required
before publishing the frictionless public flow. Installer enrollment guidance
and the compiled default origin must use the verified deployment, not an example
hostname. Existing public installer pins remain unchanged during development.
