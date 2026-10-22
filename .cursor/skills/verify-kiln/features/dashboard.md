# Dashboard

The console in `web/` is a React app served as static files. Release `v0.4.17` publishes `kiln-dashboard-v0.4.17.tar.gz`. Server `deploy/install.sh` accepts `--dashboard-archive` and installs the static tree under `/usr/local/share/kiln/dashboard` (`deploy/kiln-app.service` serves it through `kiln-api app`). Preview mode (`?preview=1`) renders `web/src/fixtures.ts` and does not call the network. Without that flag the app requests `GET /api/v1/dashboard` and, when signed in, posts operations to `/v1/operations` on the same origin (`web/src/data.ts`). All console API fetches go through `kilnFetch` in `web/src/kilnFetch.ts`: network failures (including abort/timeouts and Safari’s “Load failed”) become **Couldn’t reach the server**; HTTP errors still surface the server’s message. The live shell retries unreachable loads on **Retry connection**, the browser `online` event, and exponential backoff. `kiln-app` records `dashboard_unreachable` in its audit log when a signed-in `GET /api/v1/dashboard` cannot reach loopback `kiln-api`.

`kiln-host` builds the document and serves inventory on its Unix socket (`crates/kiln-server/src/host.rs`). Loopback `kiln-api` proxies read routes (`crates/kiln-server/src/gateway.rs`). The installed gateway listens on `127.0.0.1:8443` (`deploy/kiln-api.service`). `kiln-door` checks an identity JWT and forwards read traffic. `kiln-api app` on `127.0.0.1:8093` (`deploy/kiln-app.service`) serves the dashboard archive, browser login, CSRF-protected mutations, and proxies dashboard GET plus operate routes to loopback `kiln-api` with the owner’s operate-scoped JWT (`crates/kiln-server/src/app_listener.rs`).

## Who can read it

A read-scope owner JWT opens the public door. The audience is `kiln:server:` plus the enrolled server id. The door forwards `GET /api/v1/dashboard` to loopback `kiln-api` with the same `Authorization` header.

The administrator token is a 64-character hex secret read from the token file. Loopback `kiln-api` accepts it for this GET. The door and the app listener reject it for browser traffic.

A missing credential or a bogus bearer is HTTP 401, code `unauthorized`. A query string on the dashboard route is HTTP 400, code `invalid_request`.

## Who can mutate from the browser

Mutations require a signed-in operate session: cookies `__Host-kiln_app` and `__Host-kiln_csrf`, plus header `x-kiln-csrf` on `POST /v1/operations`. The app listener mints that session after OAuth against the enrolled identity origin. `POST /logout` with form field `csrf` clears it (`web/src/App.tsx` **Sign out**).

Live mutations (not preview, host `online`): create box, pause, resume, stop (optional force), start, delete, checkpoint, same-box restore, and one-shot exec (`web/src/dialogs.tsx`, `web/src/BoxDetail.tsx`). Preview, a disconnected live view, or host offline/unknown keeps controls disabled. The in-browser terminal is a separate session, documented below. It is not an operation and it does not add a public SSH proxy.

**Shared operation tracker (`trackOperation`, #104/#105).** All console mutations funnel through `trackOperation` in `web/src/data.ts` (formerly `submitOperation`). It POSTs `/v1/operations` with a random 32-hex `id`, sends `x-kiln-csrf` from `__Host-kiln_csrf`, polls `/v1/operations/{id}` with exponential backoff until the receipt leaves `running`, then either returns exec stdout/stderr on the receipt or re-fetches `GET /api/v1/dashboard` until `dashboardReflectsOperation` sees the expected box, checkpoint, or deletion. Inventory never updates optimistically. `web/src/data.test.ts` covers create, lifecycle, checkpoint, restore, delete, exec, CSRF or session rejection, and pending-dashboard messaging. `web/src/operationKinds.ts` lists the nine action tags kept in sync with `kiln_api::Action` by `dashboard_operation_action_types_match_api_action_tags`.

**Access panel (#107, door SSH).** On the Terminal tab, `Access` wraps SSH, HTTP preview, and public URL in `CapabilitySurface`. `ssh` is temporarily `not-yet` at the dashboard until the public-edge proxy for door SSH is verified; the Access panel shows **Coming soon** instead of copyable CLI hints. `httpPreview` and `publicUrl` stay `not-yet`. `web/src/BoxDetail.test.tsx` checks `data-capability-state="not-yet"` on SSH and absence of `kiln ssh` copy hints.

**Public-door SSH.** The door proxies `Upgrade: kiln-ssh` on `GET /v1/boxes/{id}/ssh` only for an operate-scope owner JWT and forwards that JWT to loopback `kiln-api`. See `features/public-door.md` and `features/guest-access.md`.

## Document

`schema_version` is `1`. Top-level `version` is the stamped server release (for example `0.4.17`); the gateway adds it when stamping the dashboard document. The web header shows `v` plus that value, or the build-stamped dashboard version when the field is absent. `observed_at` is UTC `YYYY-MM-DDTHH:MM:SSZ`. The gateway stamps `observed_at` and `workspace` after authentication. `Content-Type` contains `application/json`. `Cache-Control` is `no-store`.

`workspace` is the enrollment server id on an enrolled server, else `local`. `host.name` matches.

`host.state` is `online` when the host socket answers. When the dial fails, the gateway still returns HTTP 200 with `host.state` `offline` and empty `boxes`, `templates`, `checkpoints`, and `operations`.

When online, `checkpoints` lists snapshot metadata from the runtime store (`crates/kiln-server/src/host.rs`). `host.metrics.cpu_percent` and `host.metrics.memory_bytes` come from `/proc/stat` deltas and `/proc/meminfo` while `kiln-host` is online. `host.metrics.disk_bytes` and `host.metrics.disk_total_bytes` come from `statvfs` on the runtime store directory (the filesystem that holds box storage). `kiln-host` primes a baseline `/proc/stat` sample at startup (`metrics::Sampler::prime`) and retries briefly on each host read until jiffies move, so the first dashboard GET after a restart still returns a numeric `cpu_percent`. A running or paused isolated box reads its jailer cgroup `cpu.stat` (`usage_usec` deltas) and `memory.current`; stopped or inactive boxes keep null CPU and memory metrics. Every box with a disk image reports `metrics.disk_bytes` as the allocated on-host size of that image (`st_blocks` × 512); the console pairs it with `resources.disk_bytes` for a **host-observed** used-of-total label (image blocks, not guest filesystem use). The dashboard marks `metrics` as `available` in `web/src/capabilities.ts`; stopped or inactive boxes show **Stopped** or **Not available yet** for CPU and memory instead of em dashes. A box is listed only when the journal has its create row. Box `state` includes `running`, `paused`, `stopped`, `failed`, `creating`, `restoring`, and `unknown` (`web/src/data.ts` `boxStates`; host mapping in `crates/kiln-server/src/dashboard.rs`).

Operations are recent journal rows (100 newest by `created_at`, then `id`). Automatic journal retention never removes a live box's create row, so pruning does not drop boxes from this document. Corrupt JSON rows are skipped after the read. Rows whose operation state is unknown are omitted. Published `kind` is `create` for a create row and `unknown` for other journal rows today (`crates/kiln-server/src/dashboard.rs`). The web schema accepts any string for `kind`; `web/src/operationKinds.ts` lists the nine `kiln_api::Action` tags the console submits. Contract test `dashboard_operation_action_types_match_api_action_tags` in `crates/kiln-api/src/lib.rs` keeps that list aligned with the API. Operation `state` in the document is `running`, `completed`, or `failed`; the web schema also allows `queued` and step arrays for forward compatibility.

Error text containing `/` or `\` is dropped (`public_error`). Exec stdout and stderr are not copied into the dashboard document.

A host body larger than 1 MiB makes the gateway return HTTP 502 with code `host_unavailable` (`oversized_host_body_is_an_error` in `crates/kiln-server/src/gateway.rs`).

`web/src/data.ts` exports `dashboardSchema`. Integration test `dashboard_schema_accepts_enrolled_and_unenrolled_servers` in `crates/kiln-server/tests/dashboard_api.rs` parses enrolled online, unenrolled online, and offline gateway bodies through that schema (via `npm ci` in `web/` when needed).

## Sub-features

- Inventory at `/boxes`: search, state filters, name sort, row links.
- Box detail at `/boxes/:id` with Terminal, Exec, Previews, Checkpoints, Metrics, and Activity tabs. The Access panel's SSH capability is `not-yet` and shows **Coming soon** inside `CapabilitySurface` (copyable `kiln ssh` hints return when `ssh` is `available` again). HTTP preview and public access stay **Not available yet**.
- Capability map in `web/src/capabilities.ts` (#113): `metrics`, `terminal`, `ssh`, `httpPreview`, `publicUrl`, and `launchTiming` are each `available` or `not-yet`. `CapabilitySurface` in `web/src/ui.tsx` renders **Not available yet** for most `not-yet` entries (HTTP preview, public URL); SSH uses **Coming soon** via `notYetFallback` until door SSH works at the public edge. `terminal` is `available` for the in-browser shell session. `launchTiming` is `available`: the Metrics tab shows end-to-end milliseconds plus phase rows when the host measured them, or **Not reported** when `timing.total_ms` is null. On create/start/restore the runtime already times phases with task-local `Phase` markers (`crates/kiln-runtime/src/runtime/benchmark.rs`, `runtime.rs` `boot`); successful launches persist them on the box record’s `last_launch` via `LaunchTiming::from_phases` (storage, VMM boot/configure, guest ready, guest initialize). Phases must be taken inside the runtime’s `PHASES.scope` before `finish_launch`; otherwise only `total_ms` reaches the UI. The dashboard reads `last_launch` on the box record before journal-only totals. Regenerate CLI proof with `sh scripts/proof-launch-phase-timing.sh`. `web/src/capabilities.test.tsx` fails if an `available` capability shows placeholder dashes or empty values, or if a `not-yet` capability shows fake connected UI. `metrics` is `available` and reads live host and box telemetry from the dashboard document.
- Templates at `/templates`, including the create-box drawer.
- Operations at `/operations`.
- Settings at `/settings` and the guide at `/docs`.
- Explicit preview versus a live authenticated console.
- Sign out for the operate session.
- Release version label in the console header (`web/src/kiln-version.ts`, `web/src/App.test.tsx`). The label is a button that opens embedded release notes for the running build version (`web/src/release-notes.ts`, `web/src/ReleaseNotesDialog.tsx`, `scripts/release-notes-json.sh`, `scripts/write-release-notes.sh`). Notes are generated at dashboard build time from git history since the previous stable tag; the bundle never calls GitHub at runtime. A version with no embedded notes shows **No release notes for vX**. `web/src/App.test.tsx`, `web/tests/dashboard.spec.ts`, `scripts/test-release-notes.sh`, and `scripts/check-embedded-release-notes.sh` cover the UI and build-time embedding.
- Access panel: SSH **Coming soon** (`not-yet`); HTTP preview and public access labelled unavailable.
- Shared `trackOperation` polling and dashboard refresh after mutations.
- A terminal session on a running box. See below.

## Terminal session

The terminal is a session. It stays out of `kiln_api::Action`, `OPERATION_ACTION_TYPES` in `web/src/operationKinds.ts`, the `satisfies Record<OperationActionType, …>` tables and `setDashboardObserver` in `web/src/data.ts`, and `trackerCases` in `web/src/data.test.ts`. The contract test `dashboard_operation_action_types_match_api_action_tags` in `crates/kiln-api/src/lib.rs` compares those tags with the Rust enum.

`kiln-app` serves `GET /v1/boxes/{id}/terminal` (`crates/kiln-server/src/app_listener.rs`, `crates/kiln-server/src/terminal.rs`). The browser opens a same-origin WebSocket and sends the `__Host-kiln_app` and `__Host-kiln_csrf` cookies. `Origin` must be `https://app.dark-forge.dev`. The session must be operate scope. The server checks ownership by reading the box SSH host key, writes one audit line when the socket upgrades (`audit terminal outcome=opened box_id=…`), and bridges an OpenSSH PTY over the existing SSH upgrade. Idle limit is 15 minutes. Maximum duration is 2 hours. Binary frames are PTY bytes. A text frame `{"type":"resize","cols":N,"rows":M}` resizes the PTY. The server ends the socket with `{"type":"closed",…}`.

`capabilities.terminal` is `available`, so `TerminalPane` (`web/src/BoxDetail.tsx`) renders inside `CapabilitySurface`. Connect is enabled only when the console is live, the host is `online`, and the box is `running`. Preview, a stopped box, and an offline host leave the button disabled and the heading at `Session not connected`. The heading says `Connected` only while the socket is open. xterm.js loads when Connect is used. On the same tab, the Access panel labels SSH **Coming soon** (`ssh` is `not-yet`); HTTP preview and public access stay **Not available yet**.

**Public edge vs loopback bridge.** The browser WebSocket is same-origin on `https://app.dark-forge.dev` and is reverse-proxied by `deploy/app.Caddyfile` to `172.18.0.1:8093` (`kiln-app`). That hop uses a normal WebSocket upgrade, not `Upgrade: kiln-ssh`. It does not traverse `deploy/door.Caddyfile` (`cli.dark-forge.dev` → the door on `:8444`), which is where the broken public `kiln ssh` proxy returns HTTP 502 today. After auth, `kiln-app` opens the box by calling loopback `https://127.0.0.1:8443/v1/boxes/{id}/ssh` with `Upgrade: kiln-ssh` (`crates/kiln-server/src/terminal.rs`), so the PTY bridge does not depend on the cli-site Caddy fix. `ci-guards` test `dashboard_terminal_web_socket_uses_app_site_not_cli_door` keeps those deploy boundaries aligned with the code.

The in-repo proof does not boot Firecracker. `openssh-server` must provide `/usr/sbin/sshd`, and `/run/sshd` must exist. CI installs that before server-check.

```sh
cargo test -p kiln-server --test app_listener --locked -- --nocapture \
  terminal_rejects_unauthenticated_read_scope_bad_origin_and_other_owner
cargo test -p kiln-server --test app_listener --locked -- --nocapture \
  operate_session_gets_a_pty_echo_from_a_running_box
cargo test -p kiln-api --lib --locked -- --nocapture \
  dashboard_operation_action_types_match_api_action_tags
```

A rejection pass is HTTP 401 `unauthorized` with no cookie, HTTP 403 `insufficient_scope` for a read session, HTTP 403 `invalid_origin` for another origin, and HTTP 401 `unauthorized` for another owner. Those four do not dial the host. An operate session for an unknown box is HTTP 404 `not_found` and does not audit `opened`. A shell pass prints `kiln-term-ok` and audits `outcome=opened` with that box id.

```sh
cd web
npm test -- src/App.test.tsx src/BoxDetail.test.tsx
```

`enables Connect terminal only for a running box on a live console` covers the button. `labels SSH access as not available yet without kiln ssh command hints` covers the Access panel. Playwright on `/boxes?preview=1` still expects `Session not connected` and a disabled Connect button. That run does not open a shell.

A browser shell on a deployed console needs `kiln-app`, an operate-scoped `__Host-kiln_app` session, `Origin: https://app.dark-forge.dev`, and a running box. The Vite dev server does not proxy `/v1/boxes/{id}/terminal`. `deploy/app.Caddyfile` and the app socket proxy forward the dashboard WebSocket; they are separate from the cli door site that must be fixed before dashboard SSH hints return.


## How to get to it (user POV)

From a checkout, with Node 22:

```sh
cd web
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/boxes?preview=1`. Playwright uses port 4173:

```sh
npm run dev -- --host 127.0.0.1 --port 4173 --strictPort
```

On an enrolled server, open the dashboard origin served by `kiln-app` (via Caddy on the public name after enrollment). Sign in through the identity provider, then use `/boxes` without `preview=1`.

## Driving the live document

`$ADMIN_TOKEN`, `$OWNER_JWT`, `$BOGUS`, and `$CA` are placeholders. Do not write secrets into this file or `$VERIFY_EVIDENCE`.

Loopback administrator token (lab stack in `features/private-ca.md`):

```sh
curl --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -D - \
  https://127.0.0.1:8443/api/v1/dashboard
```

Public door, read-scope owner JWT:

```sh
curl --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $OWNER_JWT" \
  -D - \
  https://cli.dark-forge.dev/api/v1/dashboard
```

In-repo proof without a guest:

```sh
cargo test -p kiln-server --test dashboard_api --locked -- --nocapture \
  dashboard_schema_accepts_enrolled_and_unenrolled_servers
cargo test -p kiln-server --test public_door --locked -- --nocapture \
  dashboard_read_rejects_untrusted_bearers_and_forwards_the_owner_jwt
cargo test -p kiln-api --lib --locked -- --nocapture \
  dashboard_operation_action_types_match_api_action_tags
cargo test -p kiln-server --lib --locked -- --nocapture \
  oversized_host_body_is_an_error
cargo test -p kiln-server --test app_listener --locked -- --nocapture \
  a_signed_in_operate_session_reaches_the_host
```

Deploy and smoke validate live host metrics with `kiln-api self-check`. `scripts/test-deploy-host-tools.sh` and `cargo xtask test-deploy-workflow` fail if `deploy/github/*.sh` invokes `jq` or revives the removed shell `check-dashboard-metrics.sh` scraper (the v0.4.33 deploy break)., which deserializes the same `dashboard::Document` the server publishes and checks `host.metrics.cpu_percent`, `host.metrics.memory_bytes`, `host.metrics.disk_bytes`, and `host.metrics.disk_total_bytes` (running-box CPU may be null on the first read after restart; box disk is not gated). For launch timing, a **running** box with `timing: null` is a legacy box (no v2 journal finish and no stored measurement) and passes the general gate. Once the dashboard publishes a `timing` object for a running box, `timing.total_ms` must be set (from runtime phases, a v2 journal finish, or honest timestamps already present in an old operation JSON). Stopped boxes may show their last launch or `timing: null`; with zero running boxes the general gate passes before smoke creates a box. After `deploy/github/smoke.sh` creates the `kiln-smoke` box, smoke runs `kiln-api self-check --require-launch-timing kiln-smoke` **before** deleting the box. That gate requires `timing.total_ms` plus at least one measured phase field (`storage_ms`, `vmm_ms`, `restore_ms`, `agent_ms`, or `init_ms` from runtime `last_launch` / #129). Success prints one `smoke: launch timing kiln-smoke …` line to the deploy log; failure prints the self-check exit code and captured output. Legacy boxes elsewhere may still have `timing: null`. No shell JSON parsing on the host.

```sh
cargo test -p kiln-server --test dashboard_self_check --locked
sh scripts/test-check-dashboard-bundle.sh
```

## Driving it with Playwright

Pull requests that change console output should attach rerunnable proof via `scripts/proof.sh` (see [proof.md](proof.md)). For ad-hoc verification, install the browser once: `npx playwright install chromium`. Set `KILN_SCREENSHOT_DIR` before `npm run test:browser`.

```sh
cd web
KILN_SCREENSHOT_DIR="$VERIFY_EVIDENCE/dashboard" npm run test:browser
```

Preview tests keep mutations disabled and make no `/api/` requests. The inventory test opens `/boxes?preview=1`, filters and sorts rows, and on `api-dev` expects the terminal capability as `available` with a disabled Connect button. Live-route tests mock `GET /api/v1/dashboard` for loading, empty, and error states. Without preview on the dev server (no proxy), `/boxes` shows **Connect your control plane** with **Couldn’t reach the server** when the API is unreachable. Unit tests include `web/src/kilnFetch.test.ts`, `web/src/capabilities.test.tsx`, `web/src/BoxDetail.test.tsx` (SSH `not-yet` without `kiln ssh` copy hints), and `web/src/App.test.tsx` for capability surfaces and unreachable recovery. `cargo test -p kiln-server --test app_listener failed_dashboard_upstream_refresh_is_audited` covers the dashboard audit line. Proof screenshots for unreachable vs recovered states: `sh scripts/proof.sh dashboard-unreachable`. Release-notes dialog proof (header, then open notes): `sh scripts/proof.sh dashboard-release-notes`.

```sh
cd web
npm test -- capabilities.test.tsx BoxDetail.test.tsx
```

## Gotchas

- Preview data is labelled example data; it is not the local runtime inventory.
- A green preview browser run does not prove live mutations or a shell. Drive an operate session, the app_listener mutation test, or the terminal PTY test for those.
- `Connect terminal` stays disabled in preview, on a stopped box, and while the host is offline. A live console enables it when the host is online and the box is running.
- The terminal is a session. Leave it out of `OPERATION_ACTION_TYPES` and the operation tracker. Identity still issues the dashboard cookie at read scope, so a signed-in console cannot open the shell until that session can operate.
- CI job `dashboard` runs `npm test`, Prettier, `npm run build`, and `npm run test:browser` without Firecracker. The PTY echo test runs in `server-check`.
- That job caches Playwright browsers at `~/.cache/ms-playwright`. The cache key is the runner OS plus the `playwright` version resolved from `web/package-lock.json`. A hit runs `npx playwright install-deps chromium`. A miss runs `npx playwright install --with-deps chromium`. The install stops after 300 seconds and retries once so a stalled apt download does not use the whole 15 minute job timeout.
- Record status codes and field names. Leave `Authorization` and session cookies out of the evidence directory.
