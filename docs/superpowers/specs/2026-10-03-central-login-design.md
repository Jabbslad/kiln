# Central login and self-hosted server discovery

Date: 2026-10-03
Status: The owner approved GitHub/Google browser login and a Kiln-operated login/discovery service. This detailed design is proposed for review; implementation and deployment have not started.

## Outcome and scope

A fresh laptop installs Kiln and runs a command. Kiln opens a browser, the user signs in with GitHub or Google and approves the device, and the command continues against their self-hosted server. Users do not copy tokens, certificates or server URLs. A single registered server is selected automatically; multiple servers require an explicit selection, saved for future commands. A server being temporarily unreachable does not silently select a different server.

This is a thin end-to-end authentication slice, not a hosted VM platform. It includes central identity, device approval, owner-only server registration/discovery, direct authenticated server access, credential renewal/revocation, and server-scoped automation keys. Teams, invitations, tenant isolation, billing, public VM proxies and a general management dashboard are out of scope. Existing local administrator profiles remain usable for independent operation and recovery.

## User experience

- `kiln login` explicitly starts browser sign-in or reports the current account and selected server. Login never silently changes accounts or servers.
- An interactive command without credentials starts the same flow before dispatching any VM operation. It resumes the original command after successful enrollment, without replaying an already dispatched operation.
- Browser approval displays the requesting device, requested access, and a code matching the terminal. Signing in alone does not approve a device.
- If the browser cannot open, print a verification URL and user code. Do not print the secret device code or credential material.
- Noninteractive commands never launch a browser or wait for approval. They return an actionable authentication error and support an explicit automation credential file.
- One server is selected automatically. With several, prompt only in interactive mode; scripts require a saved selection or explicit server ID. With none, explain how to register a server instead of leaving a command waiting indefinitely.
- Keep explicit profile selection and direct HTTPS/private-CA profiles. Automatic login must not overwrite existing profiles or substitute a centrally discovered server for an explicitly selected local profile.
- `kiln logout` attempts to revoke the current device and clears its local credentials. If central Kiln is unreachable, clear local credentials but explicitly report that remote revocation could not be confirmed.
- Device listing/revocation and automation key creation/listing/revocation are available through authenticated CLI commands. Key creation shows the raw secret once and supports writing directly to a private file.
- Existing create/template requirements and box-ID selection remain unchanged. Omitting `--template` and resolving SSH targets by name are not required for this auth slice.

## Service boundaries

Add a separate Rust/Axum `kiln-identity` service with its own SQLite database, deployment package and tests. It must not depend on `kiln-runtime`, require KVM, or require installation on customers' VM hosts. Reuse existing Rust HTTP/database patterns without putting account data into the host operation journal.

The central service owns provider identities, browser sessions, CLI devices, server registrations, refresh credentials and automation keys. It stores server names, connection addresses and public trust material, but not VM inventories, guest files, exec output, server admin tokens, TLS private keys or CA signing keys. It never proxies VM operations or SSH and never probes arbitrary registered endpoints from its own network.

The laptop connects to central Kiln over publicly trusted HTTPS for login, discovery and credentials. It connects directly to the selected self-hosted API for templates, lifecycle, exec, SSH and file transfer. Private endpoints still require the laptop's LAN/VPN access. Browser approval is not a connectivity tunnel.

Only the central service needs the public browser-login hostname and GitHub/Google application configuration. The released CLI gets a default central origin after that origin is chosen and deployed. Development/test origins are explicitly configurable; do not invent or assume ownership of a production domain.

## GitHub and Google login

Use maintained OAuth/OIDC and JWT libraries, not handwritten cryptographic verification. GitHub uses its authorization-code flow and authenticated user API; Google uses its OIDC authorization-code flow. Request identity-only permissions, not repository access or general Google API access. Provider access tokens are used to resolve identity and then discarded rather than stored as Kiln credentials.

Bind authorization attempts to an expiring server-side browser session with unpredictable single-use state, PKCE S256, fixed callback URLs, and provider binding. Validate Google's signature, issuer, audience, expiry and nonce. Resolve GitHub identity through the authenticated API rather than trusting browser-supplied account information. Identify accounts by provider and immutable provider subject, never by email or mutable username.

GitHub and Google identities initially represent distinct accounts. Do not automatically merge accounts with matching email addresses. Cross-provider linking is deferred; the UI explains that users should use the same provider on subsequent sign-ins.

Browser sessions use opaque random cookies with Secure, HttpOnly, SameSite and host-only restrictions. All approval, revocation and registration mutations require CSRF protection and explicit POST actions. Set a restrictive content security policy and avoid third-party scripts on authentication pages. Secrets and callback query strings are excluded from access/error logs.

## CLI device sessions

Implement the OAuth device authorization grant (RFC 8628): independent high-entropy device secrets and human-readable user codes, ten-minute pending-request expiry, five-second initial polling interval, denial/expiry handling, and `slow_down` behavior. Rate-limit request creation, code lookup, approval and polling with bounded storage and expired-record cleanup. Browser shortcuts prefill the user code but still require confirmation.

Successful device enrollment issues a device-specific refresh credential. Store only its hash centrally; store the secret in a permission-protected local credential file, separate from profile metadata. Follow the existing owner-only file contract, reject unsafe paths, use atomic writes and serialize refreshes across CLI processes. Do not claim OS-keychain protection in this iteration.

Refresh credentials rotate atomically, expire after 30 days without use and at an absolute 90-day lifetime, and are independently revocable. Reuse of a spent refresh credential revokes that device's refresh family. Ambiguous refresh failures or a crash before the rotated credential is saved require a new login rather than weakening replay protection. Existing concurrent CLI processes share the refresh lock and re-read persisted credentials after acquiring it.

Central management credentials and server-access credentials have distinct audiences. Issue server-access JWTs with a five-minute lifetime, fixed algorithm, issuer, server-specific audience, account ID, device/key ID, explicit scope, issued-at and expiry. A token for one server must fail on another. Neither the central refresh secret nor a provider token is ever sent to a self-hosted server.

## Registering a self-hosted server

Provide an explicit local administrative enrollment command after install. Keep the installer noninteractive: installation prints the one-time registration action; it never waits for a browser or silently enrolls a host. The administrator approves registration from a browser on any device.

Registration requests originate from the local administrative tool, using a new high-entropy registration secret and the host's existing endpoint/CA. The central service associates a pending request with a one-time user code. Browser approval displays the server name, endpoint and trust fingerprint. The administrative tool polls with the secret, then persists the confirmed central account ID, immutable server ID and configured issuer locally. A public server IP or a guessed hostname is not proof of ownership.

Only that enrolled account may discover the server or obtain access grants for it. The self-hosted gateway independently enforces the persisted owner account ID and server ID in validated tokens. Do not allow the first arbitrary web visitor to claim an existing installation. Re-enrollment, owner replacement, changing the issuer or replacing existing trust material requires an explicit local administrative action; no automatic migration or adoption of legacy boxd installations.

A separate per-server registration credential authorizes directory changes only, never VM operations or account login. Store it privately on the server and hashed centrally. Account/device credentials cannot rewrite a registered server's address or trust material. Deregistration prevents new access grants; local administrative disabling of central auth remains available for immediate recovery.

## TLS and gateway authorization

Discovery returns the enrolled server HTTPS origin and public CA over the authenticated central connection. The client configures that CA for that server profile only and continues normal certificate-chain, expiry and hostname/IP verification. Never add it to the system/browser trust store, disable TLS checks, or follow redirects carrying credentials.

Pin the discovered trust identity in the saved profile. Changes require an explicit owner-authorized rotation and client confirmation rather than silently trusting a replacement. Existing annual leaf-certificate renewal remains an operational requirement; automatic certificate lifecycle management is not claimed by this slice.

Extend the unprivileged gateway, not the privileged host manager, with central-token validation. Its local enrollment fixes the issuer, owner and server audience. Cache signing keys obtained from the configured issuer's fixed HTTPS endpoint; never follow token-supplied `jku`/`x5u` URLs. Enforce algorithm allowlisting, expiry and at most 30 seconds of clock skew. Unknown signing keys fail closed if they cannot be safely refreshed; known cached keys permit offline validation of unexpired tokens.

The existing administrator-token path remains explicitly configured for local recovery and direct profiles. Do not silently fall back to it after a central authorization failure. Distinguish credential types and redact all of them from logs. Preserve request IDs and the existing at-most-once mutation dispatch contract.

Initially enrolled owners have administrator access to their server's existing shared inventory. This does not introduce multiple tenants or claim per-box isolation between account members.

## Revocation, automation and outages

Device/key revocation prevents further token issuance immediately: every central token exchange and authenticated management request checks current device/key and account status in the database, even when presented with an unexpired central access token. Already issued server tokens can authorize new requests for at most five minutes plus clock skew. Revocation is not retroactive cancellation: accepted operations continue, and established SSH/file-transfer streams are not forcibly closed in this slice. Document this bound rather than claiming immediate session termination.

Automation keys are scoped to one registered server, expire after 90 days by default, and exchange for short-lived server tokens. Initially offer `read` and `operate` scopes: `read` permits catalogs and box inspection, while `operate` additionally permits mutations, exec, SSH and transfers. Stored operation results require `operate` because they may contain exec output. Automation credentials cannot manage identities, register servers, create keys or mint broader credentials. Apply scope checks to every route, including upgraded SSH connections.

A central outage does not stop boxes or established connections. Cached unexpired tokens continue working; new sign-ins, discovery refresh and credential renewal fail with a clear service-unavailable message. Direct local profiles remain independent. Never extend expired access tokens to hide an outage.

Central Kiln is a trusted identity authority: compromise of its signing or discovery authority can undermine enrolled-server access and initial client trust. Minimize retained data, isolate signing/provider secrets from the database, keep encrypted backups, and document signing-key rotation and incident recovery. This is a deliberate trust/dependency trade-off for zero-configuration laptop onboarding.

## Delivery and acceptance

Implement in reviewable stages: central browser/device identity; host enrollment and scoped gateway validation; automatic CLI discovery/renewal; installer/package integration and end-to-end acceptance. Keep the existing live server, its credentials and unrelated dashboard work untouched.

Test security boundaries using differing accounts, servers, audiences and scopes, not only happy paths. Cover OAuth state/provider confusion, nonce and PKCE errors, code expiry/replay/denial, polling limits, double approval, refresh races/reuse, unsafe credential paths, cross-account discovery, unauthorized directory updates, revoked-device issuance, JWT algorithm/key confusion, CA substitution, TLS hostname mismatch, and SSH scope bypass.

Render and inspect browser sign-in, approval, denial, expiry and no-server states; test keyboard access and CSRF behavior. Protocol test fixtures can exercise errors deterministically, but do not label them real provider verification. Acceptance additionally requires actual GitHub and Google sign-in, a fresh client profile, and a disposable real Kiln server exercising lifecycle, binary exec, SSH and SFTP. Verify that losing central connectivity leaves guests running and does not allow expired credentials.

Commit, push and release verified product changes through the existing workflow and synchronize public installer pins. Publishing code is separate from deploying the central service. Choosing a production domain/hosting destination, registering provider applications, supplying secrets, and applying shared-infrastructure changes need the owner's participation/explicit approval. Never publish a default endpoint as usable before its deployment and real provider flows have been verified.

## References

- Existing single-host contracts: [platform design](2026-09-30-kiln-platform-design.md), [remote client](../../remote-client.md).
- Reference user experience: [boxd authentication](https://docs.boxd.sh/cli/authentication), [boxd API authentication](https://docs.boxd.sh/reference/grpc-api).
- [OAuth device authorization grant, RFC 8628](https://www.rfc-editor.org/rfc/rfc8628).
- [GitHub OAuth authorization](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps).
- [Google OpenID Connect](https://developers.google.com/identity/openid-connect/openid-connect), [Google identity validation](https://developers.google.com/identity/gsi/web/guides/verify-google-id-token).
