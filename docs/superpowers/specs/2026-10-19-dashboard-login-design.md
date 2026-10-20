# Dashboard login on the dark-forge.dev host

Date: 2026-10-19
Status: Proposed for review. The owner decided the host and the single-account rule on 2026-10-19. This document specifies that decision. It does not implement it.

The person opening the dashboard, the person running `kiln login`, and the person who enrolled the server are the same GitHub or Google account. The dashboard is the read-only inventory already defined in `web/`. The CLI keeps the device flow it has. The server keeps the enrollment it has.

## Changes to the approved central-login spec

The [central login spec](2026-10-03-central-login-design.md) stays in force except for the two changes named here. Both are also written in that spec's follow-on section.

The read-only dashboard is in scope. The central-login spec, in its outcome section, left a general management dashboard out of scope. Mutations from the dashboard stay out of scope, along with teams, invitations, billing, and public VM proxies.

This server is public. The central-login spec says a private endpoint still needs the laptop's LAN or VPN. That remains true for a server enrolled at a private address. The server in this document is the one on the dark-forge.dev host, and a laptop reaches it over the public internet. VM operations, SSH, and SFTP stay on the direct connection from the laptop to the server. `docs/identity-service.md` lines 10-13 describe today's direct path and the LAN or VPN requirement. The LAN or VPN sentence in that guide describes every enrolled server that is still on a private address.

These approved rules do not change.

Private-CA pinning stays. Discovery still returns one origin and one CA, and a saved profile still refuses a silent CA swap. See the central-login spec's TLS section, and `crates/kiln-client/src/profile.rs` lines 186-190.

Server-access JWTs stay at five minutes. `crates/kiln-identity/src/lib.rs` line 435 sets `exp` to `now + 300`. `crates/kiln-server/src/auth.rs` line 124 rejects a token whose lifetime is longer than 300 seconds, and line 117 allows 30 seconds of clock skew.

GitHub and Google stay distinct accounts. Local administrator profiles stay available for recovery. The directory credential still cannot authorize VM operations or account login.

The central-login status line that said implementation had not started is out of date. The identity service has shipped. `docs/identity-service.md` is the operator guide. That status correction records the shipped service. The approved-rule changes are the two host decisions above.

## Topology and hostnames

Three names, three credentials, one machine.

`https://dark-forge.dev` is the identity service. It already is. The CLI default is `DEFAULT_IDENTITY_ORIGIN` in `crates/kiln-api/src/auth.rs` lines 5-8. The process binds loopback `127.0.0.1:8091` and refuses any other address (`crates/kiln-identity/src/main.rs` line 8 and lines 36-38). HTTPS terminates at the reverse proxy described in `docs/identity-service.md` lines 39-45. That proxy's certificate and private key stay the identity service's. No other name uses that key file.

`https://app.dark-forge.dev` is the dashboard and the read API, on port 443. `app.dark-forge.dev` is a DNS-only A record to `78.46.98.73`. TLS is the existing Caddy ACME certificate. Caddy stores that certificate under its data volume, separate from `dark-forge.dev` and `cli.dark-forge.dev`. There is no operator-managed key file and no DNS-01 certificate. The kiln process does not speak ACME. This change does not request a certificate. The identity signing key stays `signing.pem` and is not a TLS key.

The public door is `https://cli.dark-forge.dev` on port 443. Caddy presents the existing ACME certificate and proxies to the door on `127.0.0.1:8444`. `kiln-api` stays on `127.0.0.1:8443` and terminates TLS with its own leaf (`deploy/kiln-api.service`). Public tcp/8443 is closed. The identity reverse proxy does not sit in front of `kiln-api`. A proxy certificate on that API would fail the pinned CA.

The leaf has to include `DNS:cli.dark-forge.dev`. The installer writes `subjectAltName=IP:{address}` and no DNS name (`deploy/install.py` line 333). Renewal text still says to keep that IP SAN (`docs/releases.md` lines 353-355). On this host the operator issues a new leaf from the retained private CA. The SAN includes `DNS:cli.dark-forge.dev` and any `IP:` name a current profile still dials. The operator replaces `/etc/kiln/tls.crt` and `/etc/kiln/tls.key` with that leaf and key. The certificate stays a leaf, `CA:FALSE` (`docs/remote-client.md` lines 166-169). The key stays owner-only and readable by `kiln-api` (`docs/remote-client.md` lines 169-170). The CA public file registered at enrollment stays `ca.crt`. `ca.key` stays mode 0600 (`deploy/install.py` lines 356-357).

This host enrolls with `--url https://cli.dark-forge.dev`. Identity stores that URL as the server origin (`crates/kiln-server/src/enrollment.rs` lines 24-25 and line 305).

Cookies do not cross these names. Identity sets `__Host-kiln_session` with `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, and no `Domain` attribute (`crates/kiln-identity/src/lib.rs` line 748). The `__Host-` prefix is host-only. A cookie set for `dark-forge.dev` is not sent to `app.dark-forge.dev`, and a cookie set for `app.dark-forge.dev` is not sent to `cli.dark-forge.dev`. The dashboard session uses a different cookie name, `__Host-kiln_app`, so a mistaken `Domain` attribute still cannot collide with the identity session.

`kiln-api` listens on `127.0.0.1:8443` (`crates/kiln-server/src/bin/kiln-api.rs`). That bind stays. There is no public `kiln-api` bind.

The dashboard listener is `127.0.0.1:8093` and speaks HTTP. It does not load a TLS certificate. Caddy cannot dial host loopback, so a socket proxy listens on `172.18.0.1:8093`, allows only `172.18.0.3/32`, and forwards to `127.0.0.1:8093`. The dashboard listener trusts `X-Forwarded-For` only when the TCP peer is loopback, the same rule identity uses for `--trusted-proxy-loopback`. The CLI listener ignores forwarded headers. The dashboard listener sets `Secure` cookies even though its own socket is HTTP, which is what identity already does on loopback.

## CLI trust

Use two listeners. The CLI keeps the private CA. The browser uses the public certificate on a different name.

A profile that carries a CA trusts only that CA. `Client::build` adds the PEM and calls `tls_built_in_root_certs(false)` (`crates/kiln-client/src/lib.rs` lines 71-74). The test `private_ca_does_not_trust_public_roots` (`crates/kiln-client/src/lib.rs` lines 274-290) shows a private-CA client failing the handshake with `example.com`, while a client with no CA completes it on public roots. That split landed in PR #1. Putting a Let's Encrypt certificate on the origin stored in an existing profile fails closed.

Saving a central profile for the same server id refuses a CA change with `server CA changed; explicit re-enrollment is required` (`crates/kiln-client/src/profile.rs` lines 186-190). Discovery cannot swap the pin quietly. The central-login TLS section already requires an explicit owner-authorized rotation and client confirmation.

The other candidate was one public listener, and a planned move of CLI profiles onto public roots. That candidate loses for this server. A leaf pin breaks when Let's Encrypt renews the certificate. Trusting public roots would drop the pin the approved spec requires. The client has no confirm-and-replace flow. It has a refusal. Building that flow, and then deleting the private listener, is a later change to an approved rule. This design does not start it.

A direct administrator profile is never replaced (`crates/kiln-client/src/profile.rs` lines 178-181). It keeps dialing the URL it was given, so that host stays on the leaf. A central profile for a different server id stays as saved. A central profile for this server id refuses a CA change (`crates/kiln-client/src/profile.rs` lines 186-190). `kiln login --server` copies the server descriptor identity returns, including its origin (`crates/kiln-client/src/main.rs` lines 464-471), when the CA bytes are unchanged (`crates/kiln-client/src/profile.rs` lines 186-190). Until that command the profile keeps the saved origin. A saved IP origin still matches a leaf that keeps the IP SAN. After `--server`, the profile dials `https://cli.dark-forge.dev`. The DNS name on the leaf is what makes that handshake succeed. Identity login builds its client with no custom CA (`crates/kiln-client/src/auth.rs` lines 264-275), so it uses public roots. That client is how the laptop reaches `https://dark-forge.dev`.

Enrollment writes two facts. `servers.origin` and `servers.ca_pem` (`crates/kiln-identity/src/store.rs` line 281) remain the CLI origin and the private CA. A new column stores the dashboard redirect URI, exactly `https://app.dark-forge.dev/oauth/callback`. The approval page shows the endpoint, the certificate fingerprint, and that redirect URI. The owner sees where authorization codes will be sent.

If a later decision moves the CLI onto public roots, the private listener stays until each profile has confirmed the new origin. Confirmation is a prompt in the CLI. A refused prompt leaves the old origin and the old CA in place. After the last profile has confirmed, the private listener can be removed. That work is not one of the three PRs below.

## Dashboard read API

`GET /api/v1/dashboard` returns schema version 1, as `dashboardSchema` defines it in `web/src/data.ts` lines 37-95. The browser calls it once, with `Accept: application/json`, `credentials: 'same-origin'`, and `cache: 'no-store'` (`web/src/data.ts` lines 108-112). The gateway assembles the JSON. It does not proxy this route through the existing `/v1` handlers.

The route requires a browser session whose account id equals `Enrollment.owner_id` (`crates/kiln-server/src/auth.rs` lines 19-24 and 119-120). There is one owner and one inventory. The platform design's milestone two starts with one administrator in one workspace. The server returns that inventory. Frontend filters are not authorization. `web/README.md` line 57 says the same thing about the checkpoint filter.

The box list is the journal's create rows, which is what `GET /v1/boxes` already walks (`crates/kiln-server/src/host.rs` lines 232-254, using `managed_ids`). A box id is not enough. The journal must own it. The response omits secrets, host paths, exec output, and the administrator token.

`observed_at` is the gateway clock in UTC, ISO-8601. `schema_version` is `1`.

`workspace` is the server name submitted to enrollment. The flag defaults to `kiln` (`crates/kiln-server/src/enrollment.rs` lines 28-29). The registration JSON sends that name (`crates/kiln-server/src/enrollment.rs` line 305). The local `Enrollment` struct does not store it. New enrollments persist `name` on the enrollment file. Old files have no `name`. For those, `workspace` is `server_id`, which is already required to be non-empty (`crates/kiln-server/src/auth.rs` lines 40-46). The host object uses the same display name. `BoxRecord.host` is `host::fingerprint()` (`crates/kiln-runtime/src/runtime.rs` line 594) and stays the compatibility check at line 656. The dashboard host label is the server name.

`host.state` is `online` when the Unix-socket read succeeds, and `offline` when it fails. `unknown` remains in the schema for a gateway that has not attempted the read.

`host.metrics` is null on every field. Nothing in the host API samples CPU, memory, or disk. `BoxView` is id, name, state, `memory_mib`, and `vcpus` (`crates/kiln-api/src/lib.rs` lines 151-157).

Box fields, and where they come from today.

| Field | Source | Null or empty for now |
| --- | --- | --- |
| `id`, `name` | `BoxRecord`, already copied into `BoxView` | Never null |
| `state` | `BoxRecord.state` when it is `running`, `paused`, `stopped`, `failed`, or `creating` | `starting` (`crates/kiln-runtime/src/runtime.rs` lines 445-449) and any other string are sent as `unknown`. Do not infer `restoring` |
| `image` | None that is safe to send. `Manifest` holds `kernel_path` and `rootfs_path` (`crates/kiln-runtime/src/image.rs` lines 19-29) | Empty string. The schema requires a string and forbids null. Do not send the paths |
| `resources` | `vcpus`, `memory_mib` times 1024 times 1024, and the byte length of the box disk file | The length is computed inside the host service. The path is not returned. If the stat fails, `resources` is null |
| `metrics` | No sampler | All three fields null |
| `created_at` | `operations.created_at` on the create row (`crates/kiln-server/src/journal.rs` line 30) | Present for every journal-owned box |
| `last_operation` | `BoxRecord.operation` | Null when the record has none |
| `last_error` | The fixed recovery sentences in `crates/kiln-runtime/src/runtime.rs` lines 503-520 | Null when the stored string interpolates a runtime error (`crates/kiln-runtime/src/runtime.rs` line 473), because that string can contain a host path |
| `restored_from` | `BoxRecord.source` | Null when the box has no source snapshot |
| `timing` | Not stored on `BoxRecord`. Benchmark phases are not per-box state | Null |

Template fields come from the prepared catalog (`crates/kiln-server/src/host.rs` lines 96-122). `TemplateView` is name, `memory_mib`, and `vcpus` (`crates/kiln-api/src/lib.rs` lines 160-164). The dashboard `id` is the snapshot id the catalog already maps. `name` is the alias. `compatible` is true because catalog load rejects an isolation mismatch (`crates/kiln-server/src/host.rs` lines 113-116). `development_only` is true when the runtime is not isolated. `version` and `description` are empty strings. The schema requires strings and the snapshot has neither (`crates/kiln-runtime/src/runtime.rs` lines 57-74). `resources.disk_bytes` is the snapshot disk length, same stat rule as a box. A template whose disk length cannot be measured is omitted. Do not invent a size.

Checkpoints are an empty array. `Runtime::snapshots` can list `snapshot.json` files (`crates/kiln-runtime/src/runtime/checkpoint.rs` lines 127-136). A `Snapshot` has an id and `source_box`, and it has no capture time. The schema requires `created_at`. A directory mtime is not that time. Omitting the objects avoids a fabricated timestamp. `size_bytes` can be measured later, once a real capture time exists.

Operations are the newest 100 journal rows, oldest omitted past that cap. The cap is a design limit so one response stays under `MAX_RESPONSE_BYTES` of 1 MiB (`crates/kiln-api/src/lib.rs` line 7) and does not ship the whole journal. The journal may hold 10,000 rows (`crates/kiln-server/src/journal.rs` line 78). Map `Running` to `running`, `Succeeded` to `completed`, and `Failed` to `failed` (`crates/kiln-api/src/lib.rs` lines 166-173). The dashboard schema has no `unknown` and no separate `queued`. Rows in `Unknown` are left out. Do not relabel them as `failed`. `kind` is `create` when `is_create` is 1. The journal stores a fingerprint of the action, not the action, so every other kind is the string `unknown`. `box_name` is the current box name, or the box id when the box file is gone. `detail` is the client error message. Drop `Outcome::Exec` and any argv. `steps` is an empty array. There is no step model.

## Static serving

The dashboard listener serves the extracted dashboard archive and the read API on one origin. `web/README.md` lines 32-35 and 59 require that layout. A deep link such as `/boxes` returns `index.html`. `/api/` is routed first. A missing `/api/` path returns a JSON 404. The SPA fallback does not apply under `/api/`. The client treats an HTML body as a missing API (`web/src/data.ts` lines 118-119).

`/login`, `/logout`, and `/oauth/callback` are gateway routes, registered before the SPA fallback. The authorization code is handled by the gateway and is never written into the HTML.

The archive is the one `scripts/release.py` already builds for kind `dashboard` (lines 34-48), named `kiln-dashboard-<version>.tar.gz`. The server package does not contain it today (`scripts/release.py` lines 64-79), and `deploy/install.py` does not mention a dashboard. The installer gains an explicit path to that archive and extracts it into a root-owned directory the unprivileged gateway can read, for example `/usr/local/share/kiln/dashboard`. The installer does not download. A host with no archive keeps the CLI API and serves no SPA.

Hashed files under `assets/` may be cached. `index.html` is `Cache-Control: no-store`. API responses stay `no-store`, which the proxy path already sets (`crates/kiln-server/src/gateway.rs` line 190).

Every response on the dashboard listener gets the header set identity already applies in `secure` (`crates/kiln-identity/src/lib.rs` lines 1181-1194).

- `Content-Security-Policy` is `default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'`.
- `Referrer-Policy` is `strict-origin`. Identity uses that value because `no-referrer` makes a browser form POST send `Origin: null` (`docs/identity-service.md` lines 147-149).
- `X-Content-Type-Options` is `nosniff`.

Geist and Geist Mono are bundled locally (`web/README.md` line 3), so the policy does not need a font host. Preview mode, `?preview=1`, stays a labelled local fixture and still makes no API call.

## Browser login

The browser is a confidential client of the identity service. The SPA does not hold the PKCE verifier, the authorization code, or a server JWT.

The person opens `https://app.dark-forge.dev`. With no session cookie, `GET /api/v1/dashboard` returns 401, and the existing disconnected screen remains. Sign-in navigates the top-level window to `GET /login` on that origin. The gateway creates a 10-minute attempt, the same lifetime as `oauth_attempts` (`crates/kiln-identity/src/lib.rs` line 738 stores `now+600`). It stores the PKCE verifier and `state` server-side, sets a host-only attempt cookie, and redirects the browser to `https://dark-forge.dev/oauth/server`.

That route is new. Today `/oauth/{provider}` accepts `github` and `google` only (`crates/kiln-identity/src/lib.rs` lines 83-84 and 680-683). Device login is a different grant, and `client_id` must be `kiln-cli` (`crates/kiln-identity/src/lib.rs` line 178). The dashboard does not use the device grant and does not create a CLI device.

Identity checks the redirect URI against the value stored at enrollment. Any other redirect URI is refused. The request uses PKCE S256. The authorization code is single-use and expires in 60 seconds.

An existing identity session makes this redirect silent. The browser already has `__Host-kiln_session` for `dark-forge.dev` after a visit to login, device approval, or server approval. The navigation to `dark-forge.dev` sends that cookie. When the session's account is the enrolled owner, identity redirects back to `https://app.dark-forge.dev/oauth/callback` with the code and does not show GitHub or Google. When no identity session exists, identity shows the current provider buttons, then redirects to the same callback. When the session is a different account, identity shows a refusal and issues no code. Sign-in does not switch accounts. The central-login spec already says login never silently changes accounts.

The callback checks `state`, then exchanges the code from the server, with the PKCE verifier and a login client secret. Enrollment generates the secret and stores only its hash in the identity database. On the host the file is `/etc/kiln/login.token`, root-owned, mode 0640, group `kiln-api`. Enrollment writes `identity.json` the same way (`crates/kiln-server/src/enrollment.rs` lines 168-173). The group id is the `kiln-api` group (`crates/kiln-server/src/enrollment.rs` lines 373-375, `docs/identity-service.md` lines 63-64). `directory.token` stays mode 0600 (`crates/kiln-server/src/enrollment.rs` lines 174-179). The gateway runs as `kiln-api` (`deploy/kiln-api.service` lines 8-9) and refuses to start when its user id is root (`crates/kiln-server/src/bin/kiln-api.rs` lines 42-45). Mode 0640 lets that group read `login.token`. Mode 0600 leaves `directory.token` readable by its owner. The login secret is a different credential. The directory token remains limited to directory changes.

The token endpoint returns a server-audience JWT, `kiln:server:<server id>`, the audience `verify` already requires (`crates/kiln-server/src/auth.rs` line 115). `sub` must equal `Enrollment.owner_id`. The scope is `read`. Do not reuse `POST /v1/access`. That handler always mints `operate` (`crates/kiln-identity/src/lib.rs` line 437). The gateway checks the JWT with the existing verifier, then discards it. The session cookie is an opaque id.

The session cookie is `__Host-kiln_app`, `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, no `Domain`, `Max-Age=43200`. Twelve hours matches the identity browser session (line 748). The lifetime is absolute. A later request after expiry is 401. The SPA already treats any failed dashboard response as disconnected and clears stale inventory (`web/README.md` line 55).

`SameSite=Lax` is required for the attempt cookie. The callback is a top-level navigation from `dark-forge.dev`, and Lax sends the cookie on that GET. `Strict` would drop it.

Logout is `POST /logout`. It checks `Origin` against `https://app.dark-forge.dev`, checks the CSRF value, deletes that one session row, and clears the cookie with `Max-Age=0`. Identity logout is the pattern (`crates/kiln-identity/src/lib.rs` lines 137-164). A GET does not log anyone out.

CSRF for later writes is stored now and unused by any mutation. The session row holds a CSRF hash. A second cookie, `__Host-kiln_csrf`, is `Secure` and `SameSite=Lax` and is not `HttpOnly`, so a future page can echo it. Writes, when they exist, require POST, an `Origin` match, and that value. This slice's only POST that consumes it is logout. `web/README.md` line 61 requires CSRF before cookie-authenticated writes. Those writes are still out of scope.

The dashboard cookie is not a bearer token. `authenticate` reads `Authorization` only (`crates/kiln-server/src/gateway.rs` lines 58-62). The CLI listener does not accept `__Host-kiln_app`. SSH and operate routes stay on the CLI listener, behind the bearer check they have today (`crates/kiln-server/src/gateway.rs` lines 41-48).

## Revocation

Each credential ends on its own.

Revoking a CLI device updates that device (`crates/kiln-identity/src/lib.rs` lines 458-470) and stops new tokens for it. Other devices, automation keys, identity browser sessions, and dashboard sessions stay. `kiln logout` revokes the current device and clears the local credential. It does not clear a dashboard cookie.

Logout on `app.dark-forge.dev` deletes that one dashboard session. Logout on `dark-forge.dev` deletes that one identity browser session (`crates/kiln-identity/src/lib.rs` lines 150-152). Neither logout deletes the other.

Revoking an automation key updates that key (`crates/kiln-identity/src/lib.rs` lines 1124-1132). It does not touch devices or dashboard sessions.

Stopping the account or the server stops new credentials. An already issued server JWT still verifies until `exp`, plus the 30 second leeway (`crates/kiln-server/src/auth.rs` line 117). The verifier rejects a lifetime longer than 300 seconds (`crates/kiln-server/src/auth.rs` line 124). That is at most five minutes of token life plus skew. The gateway does not ask identity on each `/v1` request. This design does not shorten that bound.

A dashboard session is a local cookie. The five-minute JWT bound does not apply to it. The gateway asks identity at most every five minutes whether the owner and the server are still active. An inactive answer deletes the dashboard sessions for that server on the next check, at most five minutes later. When identity cannot be reached, an unexpired session lasts until its 12 hour expiry. The central-login outage section keeps cached unexpired access working and does not extend an expired credential (`docs/superpowers/specs/2026-10-03-central-login-design.md` line 83).

There is no shipped command that sets `accounts.revoked_at`. Issuance checks the column (`crates/kiln-identity/src/lib.rs` line 271 and the device query at line 324). The only writer in the tree is a test (`crates/kiln-identity/tests/lifecycle.rs` line 352). Server deactivation is the shipped all-sessions action. `DELETE /v1/servers/{id}` with the directory token sets `active=0` (`crates/kiln-identity/src/lib.rs` lines 1064-1084). New grants stop because `POST /v1/access` and key exchange require `active=1`.

The check calls `POST /v1/server-status` with the login client secret. PR B adds that route. It reports whether `accounts.revoked_at` is null and whether `servers.active` is 1. It returns no CA, token, or inventory.

Removing local enrollment, or changing `owner_id`, deletes the dashboard session rows in the same administrative step. Those sessions must not survive a server that no longer has an owner.

A sign-out-everywhere action is out of scope. It would be one control that revokes every device, every automation key, every identity browser session, and every dashboard session. Those revocations already exist one credential at a time. This slice adds no combined button.

## Internet exposure

Browsers and laptops reach this host on port 443. The CLI listener and the dashboard listener bind loopback. Public tcp/8443 is closed. The default loopback bind stays for every other install.

The gateway limits how many requests run at once. It holds 32 request permits and 32 SSH session permits (`crates/kiln-server/src/gateway.rs` lines 38-39) and returns 429 when the request permits are gone (lines 139-145). The host service holds 8 request permits (`crates/kiln-server/src/host.rs` line 130). One client can take the whole gateway budget. Both public listeners add a limit of 60 requests per minute per client IP, counted before the semaphore. Login start and the OAuth callback use a tighter limit of 10 per minute per IP, matching identity's OAuth budget (`crates/kiln-identity/src/lib.rs` lines 692-697). Identity's own limits stay. They do not cover the kiln gateway.

The journal cap is unchanged. `Journal::accept` refuses a new row once 10,000 rows exist (`crates/kiln-server/src/journal.rs` lines 73-80). Nothing in the journal deletes old rows. A dashboard read does not insert one. A public `operate` credential on the CLI listener still can, and a full journal refuses new mutations with the existing error. This design does not add retention. The error already tells the operator to archive the installation.

The host has to report `development_ready: true` from `kiln-runtime doctor`. `host::evaluate` requires x86_64, `/dev/kvm` read-write, cgroup v2, Firecracker exactly 1.17.0, and a matching jailer (`crates/kiln-runtime/src/host.rs` lines 22-40). `host::check` opens `/dev/kvm` (`crates/kiln-runtime/src/host.rs` lines 78-89). `evaluate` leaves `isolated_ready` false (`crates/kiln-runtime/src/host.rs` line 41). `Runtime::create` returns the doctor problems when `development_ready` is false (`crates/kiln-runtime/src/runtime.rs` lines 562-564). A cloud VM can still prove the HTTP design when that report is false. It cannot prove a guest. This design does not weaken the check.

## Out of scope

Dashboard mutations stay out. `web/README.md` lines 61-62 require durable idempotency, resource reservation, CSRF on cookie writes, lifecycle and audit records, and observed-result reconciliation before any of those controls are enabled. The buttons in the SPA stay disabled. A timeout remains an unknown outcome.

Relaying boxes that run on other hosts stays out. This server shows the inventory of the host it is installed on. The identity service still stores no VM inventory.

Also out of this document. Moving CLI profiles onto public roots. An account-removal command. A sign-out-everywhere button. Journal retention past the 10,000 row cap. Teams, invitations, billing, and a public VM proxy.

## Delivery

Three PRs, in this order. Each one is usable before the next. None of them flips the default `kiln-api` bind off loopback. Real GitHub consent, real Google consent, and the real host certificate are Jamie's checks in every PR. A cloud VM uses the local HTTPS fixtures the identity tests already use (`docs/identity-service.md` lines 143-144) and does not describe that fixture as provider consent.

### A. Dashboard endpoint and static files

Land the assembler, `GET /api/v1/dashboard`, and static serving on the existing gateway. Authorize the route with the current bearer check, read scope, and the enrolled owner. The SPA still sends no `Authorization` header, so a browser without preview mode stays on the disconnected screen. That is the current client. Do not add a temporary token to the SPA.

A reviewer on a cloud VM can prove all of the following without a provider account.

- A read-scope token for the enrolled owner returns schema version 1. A token for another account, another audience, or no token returns 401 or 403.
- `metrics`, `timing`, and `checkpoints` match the null and empty rules above. A box state of `starting` is `unknown`. An `Unknown` operation is absent. An exec result is absent from `detail`.
- `/api/v1/dashboard` is JSON. A deep link returns `index.html`. A missing `/api/` path does not return `index.html`.
- Response headers include the content security policy, `nosniff`, and `strict-origin`. `index.html` is `no-store`.
- The installer extracts a fixture archive when given a path, and a host with no archive still serves `/v1/templates`.

Jamie checks the live name after the Caddy runbook, not in this change. This change does not request a certificate. The identity edit is app-signin.

### B. Identity-service web sign-in

Land the server authorization-code grant, the stored redirect URI, PKCE, the `read` JWT, and `POST /v1/server-status`. The server row stores the hash of the login secret. The status route takes that secret and the server id. It reports whether `accounts.revoked_at` is null and whether `servers.active` is 1. It returns no CA, token, or inventory. The approval page shows the redirect URI. No dashboard process is required.

A reviewer on a cloud VM can prove all of the following with the fixture provider.

- A redirect URI other than the enrolled one is refused. A reused code is refused. A wrong PKCE verifier is refused. A code older than 60 seconds is refused.
- An identity session for the enrolled owner returns a code without a provider page. A session for a different account returns a refusal and no code. No session shows the provider page, then returns a code.
- The exchanged JWT has audience `kiln:server:<id>`, scope `read`, and `sub` equal to the owner. `verify` accepts it. A JWT minted for another server fails `verify`.
- The directory token cannot exchange this code. The login secret cannot update the server origin or CA.
- `POST /v1/server-status` with the login secret returns both flags true for an active owner and an active server. A fixture that sets `revoked_at`, or a server with `active=0`, returns the matching flag false. The directory token is refused. A wrong login secret is refused. The body has no `ca_pem` and no access token.

Jamie checks a real GitHub consent and a real Google consent against the deployed identity service, including the silent path after an ordinary visit to `https://dark-forge.dev`.

### C. Gateway browser sessions

Land the dashboard listener, the login redirect, the callback, the session cookie, logout, and the five-minute call to `POST /v1/server-status`. Enrollment writes `/etc/kiln/login.token` root-owned, mode 0640, group `kiln-api`, and the callback reads that file. Move `GET /api/v1/dashboard` on that listener from bearer auth to the cookie. The CLI listener keeps bearer auth and does not serve the SPA.

A reviewer on a cloud VM can prove all of the following against a fixture identity service and a test certificate.

- The session cookie is `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, and has no `Domain`. The attempt cookie survives a cross-site top-level callback. Logout is POST, checks `Origin` and CSRF, and clears one session.
- After login, the SPA's same-origin `GET /api/v1/dashboard` returns the schema. With no cookie it returns 401 and the disconnected screen. Preview mode still makes no request.
- The dashboard cookie is ignored on the CLI listener. An `operate` route there still requires a bearer token.
- Deactivating the server, or removing local enrollment, ends dashboard sessions. A fixture that sets `revoked_at` ends them on the next status check. An identity process that refuses connections leaves an unexpired session usable.
- A burst past 60 requests per minute from one address is limited. The 32-permit ceiling still returns 429 under concurrency.

Jamie checks the live names. `app.dark-forge.dev` presents the Caddy ACME certificate for that name. `dark-forge.dev` presents the identity certificate. `https://cli.dark-forge.dev` on port 443 presents the Caddy certificate. Public tcp/8443 stays closed. A profile enrolled at `https://cli.dark-forge.dev` still completes `kiln templates`. `kiln-runtime doctor` reports `development_ready: true`, which requires x86_64, `/dev/kvm` read-write, cgroup v2, Firecracker exactly 1.17.0, and a matching jailer (`crates/kiln-runtime/src/host.rs` lines 22-40). The cloud VM cannot produce the live certificates or that doctor report.

## Open questions

The names `app.dark-forge.dev` and `cli.dark-forge.dev` are the defaults above. Other names on the same domain work if the certificates, the redirect URI, and the CLI origin all change together.

`Unknown` journal rows are omitted. The dashboard schema has no state for them. Add that state before PR A if those rows should appear.

Checkpoints are empty until a snapshot record has a capture time. Say if PR A should wait for that field before it returns checkpoints.

No operator command sets `accounts.revoked_at`. Server deactivation and local un-enrollment are the actions that end every session in these PRs. Say if an account-removal command belongs in one of them.

Public tcp/8443 is closed. `kiln-api` stays on `127.0.0.1:8443`. The binary default stays loopback.
