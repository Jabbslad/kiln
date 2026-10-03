# Central Login and Server Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Prefer inline execution in the checkout containing the work; tool/developer delegation restrictions take precedence over a skill's recommendation to delegate.

**Goal:** A fresh Kiln laptop signs in with GitHub or Google, approves its device, discovers its owner's self-hosted server, and uses it without copying URLs, certificates or tokens.

**Architecture:** A separate Rust identity service handles browser identity, device grants and server discovery. The existing unprivileged gateway verifies short-lived server-specific access tokens and the locally enrolled owner; management, SSH and file transfer remain direct to the self-hosted server. Existing direct administrator profiles remain independent recovery paths.

**Tech Stack:** Rust 1.95.0/edition 2024, Axum 0.8, Tokio, reqwest 0.12, rusqlite 0.38, oauth2 5.0.0, openidconnect 4.0.1, jsonwebtoken 10.4.0 with RustCrypto, server-rendered HTML, and the existing Rust/Python/shell release tests.

## Global Constraints

- Implement the [approved design](../specs/2026-10-03-central-login-design.md); no teams, invitations, billing, hosted VM scheduling or unrelated dashboard integration.
- Ten-minute pending device requests; five-second initial polling interval; RFC 8628 `slow_down` increases the interval by five seconds.
- Refresh expiry: 30 days idle, 90 days absolute; rotation and spent-token reuse detection are mandatory.
- Server JWT lifetime: five minutes; maximum accepted clock skew: 30 seconds.
- Automation keys: one server, `read` or `operate`, 90-day default expiry, no account/credential administration.
- Owner-only access to each enrolled server's current shared inventory; this does not establish hosted tenant isolation.
- Keep Rust host/runtime dependencies out of the identity service and portable client.
- Preserve the operation journal, guest protocol, VM disks/templates and at-most-once dispatch contract.
- Keep the public installer noninteractive. Server enrollment is a separate explicit operator action.
- No TLS bypass, system trust-store changes, credentials in argv/logs, credential-carrying redirects, or automatic merging by email.
- Preserve unrelated `web/` and `docs/superpowers/plans/2026-10-01-dashboard.md` work.
- Implement and verify locally; no live server restart, production identity deployment, provider registration or shared-state migration without specific authorization.
- Store review evidence in locally ignored `.amp/in/artifacts/`; never publish runtime state or secrets.
- Real GitHub/Google and real KVM acceptance are required before claiming end-to-end delivery. Deterministic protocol fixtures are not substitutes for those checks.

---

## Execution boundaries and source map

The tasks below form one dependent feature, not independently deployable services to build in parallel. Commit reviewable stages; do not publish an unfinished central endpoint as a working default. Complete all local work before requesting production deployment approval.

| Owner | Existing source | Planned additions |
| --- | --- | --- |
| Wire contracts | `crates/kiln-api/src/lib.rs` | `crates/kiln-api/src/auth.rs` |
| Central accounts, sessions, grants | New crate | `crates/kiln-identity/{Cargo.toml,src/{lib.rs,main.rs,config.rs,store.rs,device.rs,providers.rs,browser.rs,tokens.rs,servers.rs,keys.rs},tests/{device.rs,providers.rs,tokens.rs,servers.rs,keys.rs},templates/{layout.html,login.html,approve.html,result.html},static/auth.css}` |
| Server authorization/enrollment | `crates/kiln-server/src/{gateway.rs,lib.rs,bin/kiln-api.rs}` | `crates/kiln-server/src/{auth.rs,enrollment.rs}`, `crates/kiln-server/tests/{central_auth.rs,enrollment.rs}` |
| Laptop credentials/onboarding | `crates/kiln-client/src/{profile.rs,lib.rs,main.rs,ssh.rs}` | `crates/kiln-client/src/{auth.rs,credentials.rs}`, `crates/kiln-client/tests/{auth.rs,credentials.rs}` |
| Installation and packages | `deploy/install.py`, `scripts/{release.py,test-release.py,test-install.py,test-bootstrap.py}`, `.github/workflows/build.yml` | `deploy/kiln-identity.service`, `deploy/identity.example.json` |
| Operator/user documentation | `README.md`, `docs/{remote-client.md,releases.md}`, `deploy/bootstrap/README.md` | `docs/identity-service.md` |
| Real acceptance | Existing `crates/kiln-server/tests/lifecycle.rs`, `scripts/test-ssh-pty.py` | `scripts/test-central-login.py` |

Do not split small files further merely to mirror this table. `store.rs` owns migrations and transaction access; request-specific state transitions remain beside the owning device/server/key handlers. No general repository layer, identity microservices or plugin framework.

### Existing contracts to preserve

- `gateway::router(socket: &Path, token: &str) -> anyhow::Result<Router>` constructs the current administrator-only gateway. Keep it as a compatibility constructor and add `router_with_auth(socket: &Path, auth: Authenticator) -> anyhow::Result<Router>`.
- `Client::new(url: &str, token: &str, ca_pem: Option<&[u8]>) -> anyhow::Result<Client>` validates legacy 64-hex administrator tokens. Keep it; introduce `Client::with_bearer(url: &str, token: &str, ca_pem: Option<&[u8]>) -> anyhow::Result<Client>` for bounded opaque/JWT bearer values using the same secure transport construction.
- Current `Profile` serializes `url`, `token_file`, `ca_file`. Continue reading and writing this exact direct-profile shape. Add a distinct centrally enrolled profile variant rather than reinterpreting an admin token as a refresh credential.
- `ssh::proxy_command` launches a second `kiln --profile ... ssh-proxy` process. It must reload the same saved profile/credentials, share the refresh lock and never open a browser or write status messages to its protocol stdout.
- Installer update accepts an exact existing API unit command with 11 arguments. Avoid changing that command or the host service's `KillMode=process` contract.
- `scripts/release.py` uses explicit package allowlists. Identity packaging must not include a server image, credentials, checkout contents or customer configuration.

### Protocol and persistence decisions

All identity API routes are relative to the configured central HTTPS origin. Browser routes are `/login`, `/oauth/github`, `/oauth/google`, `/oauth/{provider}/callback`, `/device`, and `/device/approve`. Browser paths never accept bearer credentials in query strings. Device verification may include its public user code in the URL; OAuth callbacks contain short-lived codes and must have their query strings redacted from logs.

| Method/path | Authentication | Contract |
| --- | --- | --- |
| `POST /oauth/device/code` | Public, rate-limited | Form `client_id=kiln-cli`, bounded `device_name`; returns device response below |
| `POST /oauth/token` | Device secret or refresh secret, according to grant | RFC 8628 device grant or OAuth refresh grant; returns a rotating device credential and central access token |
| `GET /v1/me` | Central device access token | Stable account ID and device ID; provider display identity is informational |
| `GET /v1/servers` | Central device access token | Owned server descriptors only |
| `POST /v1/access` | Central device access token | JSON server ID; returns five-minute server JWT after live owner/device/server checks |
| `GET /v1/devices` | Central device access token | This account's device IDs, labels, last use and expiry; no secrets |
| `DELETE /v1/devices/{id}` | Central device access token | Revoke only this account's device; current-device revocation is logout |
| `POST /v1/registrations` | Public, rate-limited | Local operator sends endpoint, CA and name; returns separate registration secret/user code |
| `POST /v1/registrations/poll` | Registration secret | Pending/denied/expired/completed; completed response includes owner/server IDs and directory credential |
| `POST /v1/registrations/activate` | Directory credential | Mark enrollment usable after operator persists local trust and gateway readiness is checked |
| `PUT /v1/servers/{id}` | Matching directory credential | Owner-authorized metadata/trust update; increments descriptor revision |
| `DELETE /v1/servers/{id}` | Matching directory credential | Deregister; no further server-token issuance |
| `GET, POST /v1/keys` | Central device access token | List or create this account's server-scoped automation keys |
| `DELETE /v1/keys/{id}` | Central device access token | Revoke this account's automation key |
| `POST /v1/key-token` | Automation key | Return a server JWT with that key's fixed server/scope and that server's descriptor; no other servers or central credential |
| `GET /.well-known/jwks.json` | Public | Current/retiring public ES256 keys only; no private key material |

Wire types live in `kiln_api::auth`. Serde rejects unknown fields on input. Secrets do not derive `Debug`; error responses use fixed codes, not serialized request contents. Unix timestamps are seconds, IDs are opaque, and server API origins have no username, password, path, query or fragment.

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope { Read, Operate }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerDescriptor {
    pub id: String,
    pub name: String,
    pub origin: String,
    pub ca_pem: String,
    pub revision: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTokens {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub refresh_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    pub credential_id: String,
    pub scope: Scope,
}
```

Use `kiln:server:<server-id>` as the exact server audience. Central access tokens are separate opaque 256-bit random secrets, hashed in the database, with a five-minute lifetime; they are never server JWTs. SHA-256 hashing is suitable for these uniformly random bearer secrets, not passwords. Use a CSPRNG and URL-safe encoding for all secrets.

Persistent tables: `accounts`, `provider_identities`, `browser_sessions`, `oauth_attempts`, `pending_devices`, `devices`, `refresh_tokens`, `central_tokens`, `pending_registrations`, `servers`, `automation_keys`, `rate_limits`, and `audit_events`. Enforce foreign keys and unique `(provider, subject)`; use transactions for every consume/rotate/approve. Keep spent refresh hashes until their family's absolute expiry. Audit only event kind, actor/resource IDs, outcome and timestamp. Expired pending/browser/token records and old audit records are bounded and pruned; credentials are never included in audit detail.

## Task 1: Implement durable device authorization and refresh state

**Files:** Create the identity crate, `config.rs`, `store.rs`, `device.rs`, `tests/device.rs`, and `kiln-api/src/auth.rs`; modify workspace membership, `kiln-api/src/lib.rs` and lockfile.

**Interfaces:** `Store::open(path: &Path) -> anyhow::Result<Store>` opens its independent database with foreign keys, WAL and full synchronous writes. `kiln_identity::router(config: Config, store: Store) -> anyhow::Result<axum::Router>` exposes the central routes; wall time is injectable into store transitions for deterministic expiry tests. `Config` owns the public origin, storage paths and provider/signing-key file paths, not secret values printed by `Debug`.

- [ ] Add contract serialization tests and device-transition tests before handlers. Verify human code differs from secret, response contains `expires_in=600` and `interval=5`, an unapproved request returns `authorization_pending`, polling early returns `slow_down`, denial/expiry stops issuance, approval is single-use, and no result is returned to a different secret. Check both `expires_at-1` and `expires_at` with an injected clock.
- [ ] Run `cargo test -p kiln-api` and `cargo test -p kiln-identity --test device`; first execution must fail because the new contracts/transitions do not exist, not because fixtures are malformed.
- [ ] Add the minimal schema and device routes. A pending row has hashed device secret, normalized public code, request purpose, expiry, next permitted poll, interval, approval account and state. Generate 12-character human codes from an unambiguous alphabet; perform approval only through authenticated CSRF-protected handlers introduced in Task 2. Tests may seed a real store transaction, but no production route may approve an arbitrary account ID from the request.
- [ ] Implement refresh consumption with a transaction: load family and token, check account/device status and idle/absolute expiry, reject spent tokens by revoking their family, mark the old token spent, insert a fresh hashed token and central access token, then commit. With two independent SQLite connections, only one simultaneous consume may issue a response; the other detects replay and revokes the family. Recheck family status on every central request.
- [ ] Add request-body bounds (16 KiB excluding registration CA), input-length bounds, IP and account limits, `Cache-Control: no-store`, durable expiries and cleanup. Default limits: five failed user-code attempts per IP/minute and 20 per code lifetime; 10 new device/registration requests per IP/minute; cap total pending requests at 10,000 with explicit 429 responses. Only trust forwarded client addresses from a configured loopback reverse proxy; never accept arbitrary `X-Forwarded-For`.
- [ ] Re-run device tests including process/store reopen, secret-not-in-database assertions, rate-limit boundaries, deleted/revoked device checks and 30/90-day expiry. Commit `feat: add durable identity device grants`.

Dependencies are pinned through `Cargo.lock`. Use published `oauth2 = "5.0.0"`, not 5.1 from the upstream development branch. Initial dependency features:

```toml
oauth2 = { version = "5.0.0", default-features = false, features = ["reqwest", "rustls-tls"] }
openidconnect = { version = "4.0.1", default-features = false, features = ["reqwest", "rustls-tls"] }
jsonwebtoken = { version = "10.4.0", default-features = false, features = ["use_pem", "rust_crypto"] }
```

Start shared-contract tests in `crates/kiln-api/src/auth.rs` with these complete cases; add the state-machine tests in the identity crate, where their clock and persistence belong:

```rust
#[cfg(test)]
mod tests {
    use super::{Scope, ServerClaims};
    use serde_json::json;

    #[test]
    fn scope_is_explicit_and_cannot_become_administrator() {
        assert_eq!(serde_json::from_str::<Scope>("\"read\"").unwrap(), Scope::Read);
        assert_eq!(serde_json::from_str::<Scope>("\"operate\"").unwrap(), Scope::Operate);
        assert!(serde_json::from_str::<Scope>("\"admin\"").is_err());
        assert!(serde_json::from_str::<Scope>("null").is_err());
    }

    #[test]
    fn server_claims_require_audience_and_credential_identity() {
        let value = json!({
            "iss": "https://identity.example.test",
            "sub": "owner-a",
            "aud": "kiln:server:server-b",
            "iat": 1900000000_u64,
            "nbf": 1900000000_u64,
            "exp": 1900000300_u64,
            "credential_id": "device-c",
            "scope": "read"
        });
        let claims: ServerClaims = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(claims.aud, "kiln:server:server-b");
        assert_eq!(claims.scope, Scope::Read);
        for field in ["aud", "credential_id", "scope", "exp", "iat", "nbf"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<ServerClaims>(missing).is_err(), "{field}");
        }
    }
}
```

## Task 2: Add real GitHub/Google browser login and explicit approval

**Files:** Create `providers.rs`, `browser.rs`, `tests/providers.rs`, the four HTML templates listed in the source map and `static/auth.css`; modify identity router/config/store.

**Interfaces:** `ProviderIdentity { provider: Provider, subject: String, display_name: String }`, where `Provider` is an enum with `Github` and `Google`. `providers::begin` creates the provider URL and stores a one-use attempt; `providers::finish` returns only a validated `ProviderIdentity`. Browser handlers create/rotate the opaque browser session before redirecting to the original pending approval, not to an arbitrary return URL.

- [ ] Write HTTPS-provider protocol fixtures and adversarial tests for wrong/missing state, state reuse, changed browser cookie, wrong provider callback, expired attempts, failed code exchange, invalid Google signature/issuer/audience/nonce/`at_hash`, and two identities sharing an email. Assert separate account IDs for GitHub subject `101` and Google subject `101`, and no pending device approval from sign-in alone.
- [ ] Run `cargo test -p kiln-identity --test providers`; verify missing behavior fails. Tests must use real OAuth/OIDC libraries and signed fixture tokens, not replace verification with a function returning a user.
- [ ] Implement GitHub `BasicClient` authorization with `CsrfToken`, `PkceCodeChallenge::new_random_sha256()`, exact callback URI, secret from a private file and `request_async(&http_client)`. Resolve `/user` with that access token; use the numeric immutable `id` as subject. Request no repository or email-reading scopes.
- [ ] Implement Google `CoreProviderMetadata::discover_async`, `CoreClient::from_provider_metadata`, authorization-code flow with `Nonce` and PKCE, then `id_token.claims(&client.id_token_verifier(), &nonce)`. If `at_hash` is present, compare it with `AccessTokenHash::from_token` using the returned access token and verified signing key. Discard provider tokens after identity resolution. Production provider endpoints are fixed; injectable endpoints exist only in test construction, not through user-controlled issuer URLs.
- [ ] Use a bounded no-redirect, no-proxy reqwest client with a 10-second connect and 30-second overall timeout. Consume the OAuth state transaction before exchange; failed/ambiguous callbacks require a fresh attempt. Store PKCE verifier/nonce only for the ten-minute attempt lifetime in a private database, deleting them on consumption/expiry.
- [ ] Render GitHub/Google buttons and explicit device/server approval. Escape device names, server names and provider display strings. Use `__Host-kiln_session`, `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`; browser sessions expire after 12 hours. Revoke server-side state on browser logout. Approval/denial/logout require a session-bound CSRF token and POST; compare request origin to configured origin. Use CSP `default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'`, `Referrer-Policy: no-referrer`, and no external scripts/fonts/analytics.
- [ ] Follow frontend-design for implementation and Painter for initial visual exploration; keep these small auth pages separate from the unrelated `web/` app. Use the installed browser workflow to exercise login, code confirmation, denial, expired/reused code and no-server states. Capture and inspect screenshots plus keyboard/DOM checks. This validates our pages, not real provider acceptance.
- [ ] Run the provider suite and `cargo test -p kiln-identity`; commit `feat: add GitHub and Google device approval`.

## Task 3: Issue bounded server tokens and server-scoped automation keys

**Files:** Create `tokens.rs`, `keys.rs`, `tests/tokens.rs`, `tests/keys.rs`; extend central routes/store and shared auth response types.

**Interfaces:** `tokens::Signer::from_pem(kid: &str, pem: &[u8]) -> anyhow::Result<Signer>`; `Signer::issue(claims: &ServerClaims) -> anyhow::Result<String>`; JWKS publishes the corresponding P-256 public point. Token handlers determine `sub`, `aud`, times and scope from persisted authority; no request can supply its own claims.

- [ ] Write tests that decode real signed JWTs with an independently constructed public key and assert exact issuer, audience, subject, scope and `exp-iat=300`. A user owning server A but not B must get no token for B. A deleted/revoked device with a still-unexpired central token must fail issuance and all management routes.
- [ ] Write automation tests: `read` and `operate` keys on different servers; expiry at the boundary; revoked key; another owner's key ID; secret shown only at creation; key cannot call device/registration/key-management routes. Key metadata must never include its hash or raw secret.
- [ ] Run `cargo test -p kiln-identity --test tokens --test keys` and observe the intended failures.
- [ ] Load an ES256 PKCS#8 signing key from an owner-only file outside SQLite. Set `Header::new(Algorithm::ES256)` and an operator-generated `kid`. Derive public JWK coordinates with the maintained P-256 library; never publish private parameters. Support one active signing key and explicitly configured retiring public keys, retaining retiring verification material for at least the maximum token lifetime plus skew. Reject duplicate `kid` values.
- [ ] Issue central access tokens separately as opaque secrets. Authenticate every management request against live database status. Store automation secrets hashed, enforce owner/server/scope during exchange, and record redacted audit events for device approval/revocation and key creation/revocation. Return fixed public errors for unknown/revoked secrets.
- [ ] Run all identity tests; commit `feat: scope server grants and automation credentials`.

## Task 4: Register servers without trusting first-contact visitors

**Files:** Create identity `servers.rs`, `tests/servers.rs`; add shared registration response and request types; modify device/browser approval to distinguish server enrollment from laptop access.

**Interfaces:** Registration input is `{name, origin, ca_pem}`. Its response uses a registration-specific secret and public user code; successful polling returns `{server_id, owner_id, directory_token}`. Server descriptors are visible only after explicit activation by the matching directory credential. Directory-token permissions are disjoint from central/device/key permissions.

- [ ] Write tests for two owners, two registrations, wrong poll secret, double approval, expired code, registration replay, cross-owner listing, device-token metadata modification and directory-token VM access. Verify inactive registrations are not returned by discovery or accepted by token issuance.
- [ ] Run `cargo test -p kiln-identity --test servers` and confirm failures.
- [ ] Validate HTTPS origin syntax, maximum name/origin length, a single parseable CA certificate (maximum 64 KiB), and name escaping. Do not connect to or resolve the proposed endpoint on the central server. Display the normalized origin and SHA-256 DER certificate fingerprint during approval. Ownership is established by account approval plus possession of the local pending secret, not by DNS/IP possession alone.
- [ ] Implement transactional approval and single consumption. After the operator persists its matching local enrollment, the directory credential activates the registration. This prevents a registration appearing ready when local persistence failed. Ambiguous final credential delivery is not silently replayed: a new explicit enrollment is required, and unactivated registrations expire after one hour.
- [ ] Restrict metadata update/deregistration to the matching directory credential. Increment descriptor revision on changes; client trust rotation still requires explicit confirmation. Audit only IDs and outcome. Prune expired registration secrets and inactive records.
- [ ] Re-run identity tests and assert no HTTP request is made to a registered loopback, link-local or private endpoint by the identity process. Commit `feat: enroll owner-bound self-hosted servers`.

## Task 5: Verify central tokens at the existing gateway

**Files:** Add server `auth.rs`, `tests/central_auth.rs`; modify `gateway.rs`, server `Cargo.toml` and `lib.rs`; retain existing gateway tests.

**Interfaces:** `Enrollment { version: u32, issuer: String, owner_id: String, server_id: String }` is the private, locally administered gateway trust file. `Authenticator` holds the legacy token hash, optional enrollment and issuer-key cache. `async fn authenticate(&self, bearer: &str, access: RequiredAccess) -> Result<Principal, Failure>` yields either local administrator or a validated enrolled-owner principal. `Principal` distinguishes `LocalAdministrator` from `EnrolledOwner { account_id: String, credential_id: String, scope: Scope }`. `RequiredAccess` is `Read` or `Operate`, assigned by route registration rather than client input.

- [ ] Add table-driven tests using two accounts, two server audiences, two signing keys and unequal token lifetimes. Reject absent/malformed credentials, wrong issuer/subject/audience/algorithm, missing claims, future `iat`, future `nbf`, expired tokens, lifetime over 300 seconds, header-supplied key URLs and unknown `kid` when discovery is unavailable. Test exact skew boundaries with a controllable clock.
- [ ] Test route coverage: `read` allows templates/list/inspect only; it rejects operation results, mutations, SSH-key retrieval and SSH upgrade before contacting the Unix socket. `operate` and local administrator preserve existing routes. Confirm neither JWTs nor administrator tokens are forwarded upstream.
- [ ] Run `cargo test -p kiln-server --test central_auth --test gateway` and observe the authorization failures before implementation.
- [ ] Configure `jsonwebtoken::Validation::new(Algorithm::ES256)` explicitly; require `exp`, `nbf`, `iss`, `aud`, `sub`, enable `nbf` checks and set leeway 30. Deserialize `iat`, scope and credential ID as required typed fields. Check local owner equality, `exp > iat`, `exp - iat <= 300` and future `iat` explicitly; library defaults alone are insufficient.

```rust
let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
validation.set_required_spec_claims(&["exp", "nbf", "iss", "aud", "sub"]);
validation.set_issuer(&[enrollment.issuer.as_str()]);
validation.set_audience(&[format!("kiln:server:{}", enrollment.server_id)]);
validation.validate_nbf = true;
validation.leeway = 30;
```

- [ ] Fetch JWKS only from the enrollment issuer's fixed `/.well-known/jwks.json`, over verified HTTPS with redirects disabled. Accept bounded ES256 signing JWKs, unique bounded `kid`, and no private components. Atomically persist validated public-key cache beside gateway state for restart/outage operation; bind it to issuer. Rate-limit unknown-key refresh to once per 60 seconds per issuer and use cached known keys without network calls. Provide explicit local cache purge for compromised-key recovery.
- [ ] Add `router_with_auth` while preserving `router` and existing legacy-token validation. Malformed configured enrollment fails startup instead of reverting to admin-only accidentally. Missing enrollment means deliberate legacy-only mode. Re-run full server tests; commit `feat: authenticate enrolled owners at the gateway`.

## Task 6: Add local enrollment without changing service-unit compatibility

**Files:** Add server `enrollment.rs`, `tests/enrollment.rs`; modify `bin/kiln-api.rs`, `deploy/install.py`, `scripts/test-install.py` and operator docs.

**Interfaces:** Add an `enroll` subcommand to `kiln-api` while preserving its existing flat serve flags and `kiln-server VERSION` output. Clap's `subcommand_negates_reqs` allows enrollment without serve flags. The gateway reads optional enrollment from `/etc/kiln/identity.json` at startup (root-owned, group `kiln-api`, mode 0640); the directory credential stays in separate root-only `/etc/kiln/directory.token` (0600), unreadable to the gateway. Tests use explicitly supplied disposable paths rather than `/etc/kiln`.

- [ ] Test all existing `kiln-api` serve invocations and version output before extending parsing. Add disposable-filesystem tests for non-root enrollment, symlink paths, foreign-owned directories, existing identity refusal, private-file permissions, denied/expired approval and a write failure before activation. No test calls the live systemd units.
- [ ] Run `cargo test -p kiln-server --test enrollment` and `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test-install.py`; observe new behavior failing without changing existing passing tests.
- [ ] Implement `kiln-api enroll --issuer <central-origin> --url <installed-HTTPS-origin> --ca-file /etc/kiln/ca.crt`. The installer prints this fully populated command; users do not construct addresses. Enrollment requires root, locks against concurrent enrollment/install, reads safe existing CA paths, prints the public approval URL/code, and polls with the undisclosed registration secret. It writes identity/credential files atomically only after approval. Report existing enrollment rather than replacing it implicitly.
- [ ] Preserve the API's exact ExecStart and all host units/state. The explicit enrollment command installs a root-owned API-only drop-in with `StateDirectory=kiln-api` and `StateDirectoryMode=0700`, providing `/var/lib/kiln-api/jwks.json` under the existing `ProtectSystem=strict` sandbox. After durable local enrollment it reloads systemd, restarts only `kiln-api.service`, checks authenticated HTTPS readiness with the local admin credential, and activates the directory record. Document the brief API/SSH disconnection in command help. Implementing this command does not authorize executing it on the live installation. Tests must use disposable services or command fixtures.
- [ ] Make activation idempotent for the same directory credential. If readiness/activation fails, retain a root-private enrollment receipt and provide `kiln-api enroll --resume` to repeat readiness/activation using that receipt, without generating a new owner or replaying browser approval. If the one-time credential response was lost before local persistence, require fresh enrollment and let the unactivated record expire. Do not report success while local and central registration states disagree.
- [ ] Add installer output for fresh installations and updates without installing or configuring the central identity service on VM hosts. Preserve existing identity/directory files and permissions through no-op updates and rollback. Adding these files must not require a guest-image rebuild or a migration of runtime state.
- [ ] Run enrollment, gateway and installer suites; commit `feat: add explicit server account enrollment`.

## Task 7: Persist central client credentials and refresh safely

**Files:** Add client `credentials.rs`, `auth.rs`, `tests/credentials.rs`; modify `profile.rs`, `lib.rs`, client `Cargo.toml` and shared auth types.

**Interfaces:** `Profile` becomes a serde-untagged choice between the exact existing `DirectProfile` and a strict `CentralProfile { issuer, server: ServerDescriptor, credential_file: PathBuf }`. `CredentialStore` owns load/atomic-save/clear and a per-account refresh lock. Export `profile`, `auth` and `credentials` from the client library, replacing the binary's `mod profile` with a library import; `profile.rs` uses `crate::Client`. `auth::Session` calls central endpoints; `async fn resolve_client(profile: &Profile) -> anyhow::Result<Client>` constructs a fixed-token direct client or a central client with a session credential source.

- [ ] Write serialization tests showing existing profile JSON round-trips unchanged; central profile JSON contains no secrets. Reject ambiguous/unknown fields, invalid origins, changed issuer and unsafe credential paths. Add file permission and symlink tests before storage implementation.
- [ ] Run `cargo test -p kiln-client --test credentials` and existing CLI tests; new tests fail while old profile tests remain green.
- [ ] Implement per-issuer/account private credential storage (owner-only directories/files, exclusive locking, no symlink following, owner verification on Unix, restrictive Windows current-user ACLs for newly created secrets). Use the existing atomic temp-file/persist/fsync pattern. Do not fix permissions on unrelated directories or write provider/central credentials into SSH config.
- [ ] Within the cross-process lock, re-read credentials, reuse unexpired central/server tokens, or rotate refresh credentials once and save the response durably before releasing the lock. On ambiguous rotation or reuse errors, invalidate the local session and require login. Never automatically retry refresh POSTs after a lost response. Keep server-token cache keyed by immutable server ID and scope; never send refresh secrets to server origins.
- [ ] Implement `Client::with_bearer` with a bounded, header-safe fixed token and mandatory existing HTTPS checks. Keep legacy `Client::new` strict. Central clients resolve their session's current server token before every management request and SSH handshake, renewing when less than 30 seconds remain. Attach the resulting header to that request rather than retaining an expired default header for the life of a command. Share the transport construction directly; do not add a retry adapter or replay a mutation on 401. Established streams remain independent of subsequent token expiry.
- [ ] Test concurrent real CLI processes against one refresh endpoint: one refresh exchange, both use the stored rotated secret, and no duplicated VM mutation. Test an operation that outlives one token: later status polls renew while the original mutation is never resubmitted. Test crash-after-refresh/lost-response as explicit re-login, expired-token outage errors, cached-token offline operation, and mismatched CA/hostname rejection. Re-run client suites; commit `feat: persist and renew per-device client sessions`.

## Task 8: Wire frictionless CLI login, discovery and credential commands

**Files:** Modify client `main.rs`, `auth.rs`, `ssh.rs`, `tests/cli.rs`; add `tests/auth.rs`; update `docs/remote-client.md`.

**Interfaces:** Add `login`, `logout`, `auth status`, `auth devices list|revoke`, `auth keys create|list|revoke`, and server selection. `--profile` becomes optional at parsing so explicit selection can be distinguished from a default. An explicit missing profile is an error, never a central-login fallback. `--auth-token-file` supplies an automation credential; secrets are not accepted as command-line values.

- [ ] Write real CLI-process tests for first-use login, existing direct profile, explicit missing profile, no servers, one server, multiple servers, unreachable selected server, browser launch failure, denied/expired approval and noninteractive stdin. Assert no VM request occurs before approval; after success exactly one operation is dispatched with one request ID.
- [ ] Run `cargo test -p kiln-client --test auth --test cli` and record intended failures.
- [ ] Implement `kiln login` polling with RFC intervals/backoff, cancellation and bounded expiry. Open the verification URL using OS process arguments (`open`, `xdg-open`, or Windows browser API), never a shell command containing user-controlled text. Print the public URL/code to stderr for remote terminals and preserve structured stdout.
- [ ] Auto-login only when stdin and stderr are terminals and the command is interactive. Never auto-login from `ssh-proxy`, automation mode or JSON scripting mode. Discover only owned active servers, auto-select only when exactly one exists, and persist explicit choices. Cache the original CA trust; reject changed fingerprints until explicit confirmation. Normal leaf renewal under the same CA does not require re-enrollment.
- [ ] Implement device/key commands with live central authorization, redacted output and private-file key output. Logout attempts server-side device revocation before local deletion, reports unconfirmed remote revocation honestly, and does not erase direct profiles or unrelated accounts.
- [ ] Preserve SSH proxy stdout as protocol bytes; refresh silently using the shared credentials and emit authentication failures only on stderr. Never reuse a different profile as fallback. Preserve guest stdout/stderr, exit statuses and mutation request IDs through onboarding.
- [ ] Run CLI, credential and SSH tests on Linux; run portable login/profile checks in the existing macOS/Windows matrix. Commit `feat: add automatic browser login and server discovery`.

## Task 9: Package the independent identity service and document operation

**Files:** Modify `scripts/release.py`, `scripts/test-release.py`, `.github/workflows/build.yml`, `README.md`, `docs/releases.md`; add `deploy/kiln-identity.service`, `deploy/identity.example.json`, `docs/identity-service.md`.

**Interfaces:** Add release package kind `identity` for Linux x86-64: `bin/kiln-identity`, example configuration, systemd unit and identity operator documentation only. No VM image or KVM dependency. Existing server/client package contents remain compatible.

- [ ] Add packaging tests for exact identity allowlist, regular-file checks, version metadata and absence of images/private runtime files. Check the existing server still contains only its designated binaries and assets.
- [ ] Run `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test-release.py`; observe the missing identity package failure.
- [ ] Add identity build/package artifact to CI and release asset collection. Build standalone `kiln-identity --version` and `--help` without runtime/KVM. Preserve existing client target matrix and package checksum verification.
- [ ] Provide a hardened unprivileged systemd unit with private configuration/state paths, no KVM access and no server admin token. Bind the app only on loopback behind an explicitly configured HTTPS reverse proxy; validate the public origin and trusted proxy settings at startup. Example configuration references secret file paths, never real credentials.
- [ ] Document public DNS/TLS and exact GitHub/Google callback configuration, signing-key generation/rotation, encrypted backup/restore, database migration backup/refusal rules, account/provider identity semantics, revocation bounds, registration/recovery, network reachability and service outages. State that an operator-approved deployment is still required. Fix only existing release documentation claims directly superseded by this feature's actual verification.
- [ ] Re-run packaging/installer/bootstrap tests; commit `build: package the Kiln identity service`.

## Task 10: Verify real browser login and direct VM access

**Files:** Add `scripts/test-central-login.py`; extend `crates/kiln-server/tests/lifecycle.rs` only where reusable real-host coverage belongs; store evidence under `.amp/in/artifacts/`.

**Interfaces:** Acceptance script takes explicit fixture origins, disposable profile directories and secret-file paths. It does not create provider accounts, expose a server publicly, stop live services or print credentials. It validates an already approved disposable test deployment; cleanup is restricted to resources it created and recorded.

- [ ] Run the complete deterministic suite first: formatting, Clippy, workspace Rust tests, installer/bootstrap/package tests and ShellCheck. Include adversarial negative paths; do not relax a security assertion to make a provider fixture pass.
- [ ] Obtain owner approval for a specific central test hostname/hosting destination and provider application registration. Supply OAuth secrets through protected files/private input, not thread messages or argv. Do not register a production default endpoint until deployment is verified. If these prerequisites are unavailable, report real-provider acceptance as blocked, not successful.
- [ ] Sign in through real GitHub and Google separately, approve real device codes, and register a disposable KVM host through the same public-facing browser flow. Browser consent is a human step; the agent must not fabricate successful approval. Confirm same-email identities are not merged.
- [ ] On a fresh remote client profile with no server address or private CA supplied, perform browser sign-in, auto-discovery and real templates/list/create/inspect. Verify binary exec stdout/stderr and a nonzero guest exit code, interactive PTY SSH, and asymmetric non-ASCII/binary SFTP in both directions. Confirm gateway sees a server JWT, not a central or provider credential, without recording token values.
- [ ] Run two clients concurrently through renewal; preserve exactly one create request. Revoke one device and one automation key, verify immediate central denial and bounded server-token validity, while another device remains usable. Confirm read-only keys cannot obtain stored exec output or SSH access.
- [ ] Stop only the disposable identity test service with approved test scope; existing guest processes/SSH continue, cached tokens work until expiry, then new API requests fail. Direct admin recovery remains functional. Restart the test service and verify persistent sessions/server enrollment and issuer-key caching.
- [ ] Inspect rendered auth screenshots and retain sanitized evidence with versions, request IDs, precise results and limitations. Clean up disposable tokens/keys/hosts and test-provider sessions without deleting user data. Commit only reusable acceptance scripts and honest documentation, never artifacts containing credentials.

## Task 11: Release the verified feature and synchronize the installer

**Files:** Workspace version/lockfile, `deploy/install.py`, `scripts/test-install.py`, `deploy/bootstrap/install.sh`, `deploy/bootstrap/README.md`, release notes and the existing workflow.

- [ ] Choose the next unused release version after inspecting remote tags. Review and test each allowed server upgrade transition; retain the exact unit/guest/state compatibility requirements. Preserve enrolled-server files and legacy admin profiles on both successful update and rollback. Use disposable local state for tests, not the live installation.
- [ ] Choose the production central origin only after explicit deployment approval, provider setup and real acceptance. Embed it in the CLI build configuration; allow an explicit development override. If no verified production origin exists, do not advertise a zero-configuration public login experience or mark this task complete.
- [ ] Run the final checks from the release checkout:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test-install.py
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test-bootstrap.py
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test-release.py
shellcheck deploy/bootstrap/install.sh scripts/fetch-firecracker.sh images/build-ubuntu.sh
git diff --check
```

- [ ] Audit the exact staged/public diff for credentials, private hostnames/IPs and runtime files. Commit, push to GitHub and Amp, create the matching release tag, and follow the existing workflow to completion. Do not overwrite a failed or published version tag.
- [ ] Download every release asset and verify checksums/contents, publish through the existing release process, update bootstrap pins only to verified assets, and synchronize only `deploy/bootstrap/install.sh` and `deploy/bootstrap/README.md` to `Jabbslad/kiln-install`. Verify the anonymous public bootstrap bytes and a fresh installation.
- [ ] Report precise delivery status: source committed/pushed, release version/assets, real auth/runtime checks, remaining deployment actions and unsupported cases. Publishing a release does not authorize restarting or enrolling the owner's live server.

## Self-review and execution status

- [x] Mapped every approved design section to an implementation/test owner.
- [x] Confirmed legacy gateway/client constructor, proxy subprocess, installer unit and release allowlist constraints in current source.
- [x] Checked OAuth/OIDC/JWT library APIs against upstream examples and published package-index versions.
- [x] Separated fixture coverage from mandatory real-provider and KVM acceptance.
- [x] Kept production domain, provider secrets and live deployment behind explicit approval rather than inventing infrastructure.
- [ ] Tasks 1–11 implemented and verified.

### Implementation checkpoint (2026-10-03)

The source implementation now includes the identity service, GitHub/OIDC adapters,
device/browser approval, owner-bound enrollment and resumable activation, gateway
ES256/JWKS verification, client login/discovery/renewal, device/key commands,
private Unix/Windows storage, and independent identity packaging. Existing admin
profiles and direct VM transport are preserved. Protocol, storage, gateway,
enrollment failure/recovery and CLI scripting tests are present; browser forms were
rendered and exercised on desktop and mobile. This is not completion of Tasks 10–11.

Implementation adjustments:

- Browser pages use `strict-origin`, not `no-referrer`. A real Chromium form POST
  under `no-referrer` sent `Origin: null` and failed CSRF origin validation.
  `strict-origin` preserves that check without leaking query strings.
- Automation uses `kiln login --auth-token-file PATH` once to create a profile;
  subsequent commands reuse its private key-file reference.
- Refresh writes an invalid-session marker before transmission, then atomically
  saves the rotation before use. This avoids relying on Windows unlink durability.
- Identity configuration uses an environment file containing only public IDs and
  origin; provider/signing secrets stay in separate protected files.

Remaining acceptance/delivery: native Windows ACL execution; real GitHub and Google
consent; an approved central HTTPS deployment and disposable host enrollment; fresh
CLI onboarding plus real VM/SSH/SFTP/PTY, concurrent refresh, revocation and outage
acceptance. A reusable real-client smoke script is provided but not yet exercised
against central-authenticated VMs. No production origin, provider apps, live server
enrollment or deployment has been created. Public origin embedding, installer
enrollment guidance and release/installer pin updates follow verified deployment.
Do not replace these acceptance requirements with fixture results or mark all plan
checkboxes complete based on this checkpoint.
