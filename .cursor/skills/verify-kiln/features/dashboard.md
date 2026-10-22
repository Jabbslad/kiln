# Dashboard

The console in `web/` is a React app served as static files. Release `v0.4.17` publishes `kiln-dashboard-v0.4.17.tar.gz`. Server `deploy/install.sh` accepts `--dashboard-archive` and installs the static tree under `/usr/local/share/kiln/dashboard` (`deploy/kiln-app.service` serves it through `kiln-api app`). Preview mode (`?preview=1`) renders `web/src/fixtures.ts` and does not call the network. Without that flag the app requests `GET /api/v1/dashboard` and, when signed in, posts operations to `/v1/operations` on the same origin (`web/src/data.ts`). All console API fetches go through `kilnFetch` in `web/src/kilnFetch.ts`: network failures (including abort/timeouts and Safari’s “Load failed”) become **Couldn’t reach the server**; HTTP errors still surface the server’s message. The live shell retries unreachable loads on **Retry connection**, the browser `online` event, and exponential backoff. `kiln-app` records `dashboard_unreachable` in its audit log when a signed-in `GET /api/v1/dashboard` cannot reach loopback `kiln-api`.

`kiln-host` builds the document and serves inventory on its Unix socket (`crates/kiln-server/src/host.rs`). Loopback `kiln-api` proxies read routes (`crates/kiln-server/src/gateway.rs`). The installed gateway listens on `127.0.0.1:8443` (`deploy/kiln-api.service`). `kiln-door` checks an identity JWT and forwards read traffic. `kiln-api app` on `127.0.0.1:8093` (`deploy/kiln-app.service`) serves the dashboard archive, browser login, CSRF-protected mutations, and proxies dashboard GET plus operate routes to loopback `kiln-api` with the owner’s operate-scoped JWT (`crates/kiln-server/src/app_listener.rs`).

## Who can read it

A read-scope owner JWT opens the public door. The audience is `kiln:server:` plus the enrolled server id. The door forwards `GET /api/v1/dashboard` to loopback `kiln-api` with the same `Authorization` header.

The administrator token is a 64-character hex secret read from the token file. Loopback `kiln-api` accepts it for this GET. The door and the app listener reject it for browser traffic.

A missing credential or a bogus bearer is HTTP 401, code `unauthorized`. A query string on the dashboard route is HTTP 400, code `invalid_request`.

## Who can mutate from the browser

Mutations require a signed-in operate session: cookies `__Host-kiln_app` and `__Host-kiln_csrf`, plus header `x-kiln-csrf` on `POST /v1/operations`. The app listener mints that session after OAuth against the enrolled identity origin. `POST /logout` with form field `csrf` clears it (`web/src/App.tsx` **Sign out**).

Live mutations (not preview, host `online`): create box, pause, resume, stop (optional force), start, delete, checkpoint, same-box restore, and one-shot exec (`web/src/dialogs.tsx`, `web/src/BoxDetail.tsx`). Preview, a disconnected live view, or host offline/unknown keeps controls disabled. The in-browser terminal tab stays disabled; do not document terminal transport here.

**Shared operation tracker (`trackOperation`, #104/#105).** All console mutations funnel through `trackOperation` in `web/src/data.ts` (formerly `submitOperation`). It POSTs `/v1/operations` with a random 32-hex `id`, sends `x-kiln-csrf` from `__Host-kiln_csrf`, polls `/v1/operations/{id}` with exponential backoff until the receipt leaves `running`, then either returns exec stdout/stderr on the receipt or re-fetches `GET /api/v1/dashboard` until `dashboardReflectsOperation` sees the expected box, checkpoint, or deletion. Inventory never updates optimistically. `web/src/data.test.ts` covers create, lifecycle, checkpoint, restore, delete, exec, CSRF or session rejection, and pending-dashboard messaging. `web/src/operationKinds.ts` lists the nine action tags kept in sync with `kiln_api::Action` by `dashboard_operation_action_types_match_api_action_tags`.

**Access panel (#107, door SSH).** On the Terminal tab, `Access` wraps SSH, HTTP preview, and public URL in `CapabilitySurface`. `ssh` is `available`: copyable `kiln ssh <full box id>` and `kiln ssh-config <full box id>` for the Linux and macOS CLI. `httpPreview` and `publicUrl` stay `not-yet`. `web/src/BoxDetail.test.tsx` checks `data-capability-state="available"` on SSH and the full-id commands.

**Public-door SSH.** The door proxies `Upgrade: kiln-ssh` on `GET /v1/boxes/{id}/ssh` only for an operate-scope owner JWT and forwards that JWT to loopback `kiln-api`. See `features/public-door.md` and `features/guest-access.md`.

## Document

`schema_version` is `1`. Top-level `version` is the stamped server release (for example `0.4.17`); the gateway adds it when stamping the dashboard document. The web header shows `v` plus that value, or the build-stamped dashboard version when the field is absent. `observed_at` is UTC `YYYY-MM-DDTHH:MM:SSZ`. The gateway stamps `observed_at` and `workspace` after authentication. `Content-Type` contains `application/json`. `Cache-Control` is `no-store`.

`workspace` is the enrollment server id on an enrolled server, else `local`. `host.name` matches.

`host.state` is `online` when the host socket answers. When the dial fails, the gateway still returns HTTP 200 with `host.state` `offline` and empty `boxes`, `templates`, `checkpoints`, and `operations`.

When online, `checkpoints` lists snapshot metadata from the runtime store (`crates/kiln-server/src/host.rs`). `host.metrics.cpu_percent` and `host.metrics.memory_bytes` come from `/proc/stat` deltas and `/proc/meminfo` while `kiln-host` is online. `host.metrics.disk_bytes` and `host.metrics.disk_total_bytes` come from `statvfs` on the runtime store directory (the filesystem that holds box storage). `kiln-host` primes a baseline `/proc/stat` sample at startup (`metrics::Sampler::prime`) and retries briefly on each host read until jiffies move, so the first dashboard GET after a restart still returns a numeric `cpu_percent`. A running or paused isolated box reads its jailer cgroup `cpu.stat` (`usage_usec` deltas) and `memory.current`; stopped or inactive boxes keep null CPU and memory metrics. Every box with a disk image reports `metrics.disk_bytes` as the allocated on-host size of that image (`st_blocks` × 512); the console pairs it with `resources.disk_bytes` for a **host-observed** used-of-total label (image blocks, not guest filesystem use). The dashboard marks `metrics` as `available` in `web/src/capabilities.ts`; stopped or inactive boxes show **Stopped** or **Not available yet** for CPU and memory instead of em dashes. A box is listed only when the journal has its create row. Box `state` includes `running`, `paused`, `stopped`, `failed`, `creating`, `restoring`, and `unknown` (`web/src/data.ts` `boxStates`; host mapping in `crates/kiln-server/src/dashboard.rs`).

Operations are recent journal rows (100 newest by `created_at`, then `id`). Corrupt JSON rows are skipped after the read. Rows whose operation state is unknown are omitted. Published `kind` is `create` for a create row and `unknown` for other journal rows today (`crates/kiln-server/src/dashboard.rs`). The web schema accepts any string for `kind`; `web/src/operationKinds.ts` lists the nine `kiln_api::Action` tags the console submits. Contract test `dashboard_operation_action_types_match_api_action_tags` in `crates/kiln-api/src/lib.rs` keeps that list aligned with the API. Operation `state` in the document is `running`, `completed`, or `failed`; the web schema also allows `queued` and step arrays for forward compatibility.

Error text containing `/` or `\` is dropped (`public_error`). Exec stdout and stderr are not copied into the dashboard document.

A host body larger than 1 MiB makes the gateway return HTTP 502 with code `host_unavailable` (`oversized_host_body_is_an_error` in `crates/kiln-server/src/gateway.rs`).

`web/src/data.ts` exports `dashboardSchema`. Integration test `dashboard_schema_accepts_enrolled_and_unenrolled_servers` in `crates/kiln-server/tests/dashboard_api.rs` parses enrolled online, unenrolled online, and offline gateway bodies through that schema (via `npm ci` in `web/` when needed).

## Sub-features

- Inventory at `/boxes`: search, state filters, name sort, row links.
- Box detail at `/boxes/:id` with Terminal, Exec, Previews, Checkpoints, Metrics, and Activity tabs. The Access panel's SSH capability is `available` and shows copyable `kiln ssh <full box id>` and `kiln ssh-config <full box id>` for the Linux and macOS CLI inside `CapabilitySurface`. HTTP preview and public access stay **Not available yet**.
- Capability map in `web/src/capabilities.ts` (#113): `metrics`, `terminal`, `ssh`, `httpPreview`, `publicUrl`, and `launchTiming` are each `available` or `not-yet`. `CapabilitySurface` in `web/src/ui.tsx` renders **Not available yet** for `not-yet` entries (terminal body, HTTP preview, public URL). `ssh` is `available` because the public door proxies `kiln ssh`. `launchTiming` is `available`: the Metrics tab shows end-to-end milliseconds plus phase rows when the host measured them, or **Not reported** when `timing.total_ms` is null. On create/start/restore the runtime already times phases with task-local `Phase` markers (`crates/kiln-runtime/src/runtime/benchmark.rs`, `runtime.rs` `boot`); successful launches persist them on the box record’s `last_launch` via `LaunchTiming::from_phases` (storage, VMM boot/configure, guest ready, guest initialize). Phases must be taken inside the runtime’s `PHASES.scope` before `finish_launch`; otherwise only `total_ms` reaches the UI. The dashboard reads `last_launch` on the box record before journal-only totals. Regenerate CLI proof with `sh scripts/proof-launch-phase-timing.sh`. `web/src/capabilities.test.tsx` fails if an `available` capability shows placeholder dashes or empty values, or if a `not-yet` capability shows fake connected UI. `metrics` is `available` and reads live host and box telemetry from the dashboard document.
- Templates at `/templates`, including the create-box drawer.
- Operations at `/operations`.
- Settings at `/settings` and the guide at `/docs`.
- Explicit preview versus live authenticated console.
- Sign out for the operate session.
- Release version label in the console header (`web/src/kiln-version.ts`, `web/src/App.test.tsx`). The label is a button that opens embedded release notes for the running build version (`web/src/release-notes.ts`, `web/src/ReleaseNotesDialog.tsx`, `scripts/release-notes-json.sh`, `scripts/write-release-notes.sh`). Notes are generated at dashboard build time from git history since the previous stable tag; the bundle never calls GitHub at runtime. A version with no embedded notes shows **No release notes for vX**. `web/src/App.test.tsx`, `web/tests/dashboard.spec.ts`, `scripts/test-release-notes.sh`, and `scripts/check-embedded-release-notes.sh` cover the UI and build-time embedding.
- Access panel: SSH copy hints (`available`); HTTP preview and public access labelled unavailable.
- Shared `trackOperation` polling and dashboard refresh after mutations.

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

Preview tests keep mutations disabled and make no `/api/` requests. The inventory test opens `/boxes?preview=1`, filters and sorts rows, and on `api-dev` expects the terminal capability as **Not available yet**. Live-route tests mock `GET /api/v1/dashboard` for loading, empty, and error states. Without preview on the dev server (no proxy), `/boxes` shows **Connect your control plane** with **Couldn’t reach the server** when the API is unreachable. Unit tests include `web/src/kilnFetch.test.ts`, `web/src/capabilities.test.tsx`, `web/src/BoxDetail.test.tsx` (copyable `kiln ssh` / `kiln ssh-config` with the full box id), and `web/src/App.test.tsx` for capability surfaces and unreachable recovery. `cargo test -p kiln-server --test app_listener failed_dashboard_upstream_refresh_is_audited` covers the dashboard audit line. Proof screenshots for unreachable vs recovered states: `sh scripts/proof.sh dashboard-unreachable`. Release-notes dialog proof (header, then open notes): `sh scripts/proof.sh dashboard-release-notes`.

```sh
cd web
npm test -- capabilities.test.tsx BoxDetail.test.tsx
```

## Gotchas

- Preview data is labelled example data; it is not the local runtime inventory.
- A green preview browser run does not prove live mutations; drive an operate session or the app_listener test for that.
- CI job `dashboard` runs `npm test`, Prettier, `npm run build`, and `npm run test:browser` without Firecracker.
- Record status codes and field names. Leave `Authorization` and session cookies out of the evidence directory.
